use crate::{
    Failure,
    irq::Timer,
    memory::user::AddressSpace,
    threads, time,
    user::{self, Counts, Report, Stop, UserContext},
};
use kutox::{
    run_queue::{RunQueue, TaskHandle, TaskId},
    user_state::{ExecutionBudget, MAX_RUN_US},
};
use moss_libkernel::task::{context::Context, elf::Loaded};

pub const CAPACITY: usize = 2;

struct Task<'a, 'storage> {
    space: AddressSpace<'a, 'storage>,
    context: Context<UserContext>,
    budget: ExecutionBudget,
    counts: Counts,
    started: bool,
}

pub struct Dispatcher<'a, 'storage> {
    tasks: [Option<Task<'a, 'storage>>; CAPACITY],
    queue: RunQueue<CAPACITY>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Event {
    pub task: TaskHandle,
    pub report: Report,
    pub elapsed_ticks: u64,
}

pub type RuntimeCallback<'scope, 'a, 'storage> =
    dyn FnMut(&mut Dispatcher<'a, 'storage>, Option<Event>) -> Result<(), Failure> + 'scope;

fn context() -> Result<(), Failure> {
    threads::check_user_dispatch_context()
        .map_err(|error| Failure::new("user_task_context", error as i32))
}

pub(crate) fn duration(usec: u64) -> Result<u64, Failure> {
    if usec == 0 || usec > MAX_RUN_US {
        return Err(Failure::new("user_task_duration", -1));
    }
    time::ticks_for_us(usec)
}

impl Default for Dispatcher<'_, '_> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a, 'storage> Dispatcher<'a, 'storage> {
    pub const fn new() -> Self {
        Self {
            tasks: [const { None }; CAPACITY],
            queue: RunQueue::new(),
        }
    }

    pub fn admit(
        &mut self,
        loaded: Loaded<AddressSpace<'a, 'storage>>,
        budget_us: u64,
    ) -> Result<TaskHandle, Failure> {
        context()?;
        let budget = ExecutionBudget::new(duration(budget_us)?)
            .map_err(|error| Failure::new("user_task_budget", error as i32))?;
        let id = self
            .queue
            .vacant()
            .map_err(|error| Failure::new("user_task_capacity", error as i32))?;
        self.queue
            .add(id)
            .map_err(|error| Failure::new("user_task_admit", error as i32))?;
        let handle = self.queue.handle(id).expect("newly admitted user task");
        self.tasks[id.0] = Some(Task {
            space: loaded.space,
            context: Context::from_user_ctx(UserContext::new(loaded.entry, loaded.stack)),
            budget,
            counts: Counts::default(),
            started: false,
        });
        Ok(handle)
    }

    pub fn cancel(&mut self, task: TaskHandle) -> Result<(), Failure> {
        context()?;
        self.queue
            .remove_runnable(task)
            .map_err(|error| Failure::new("user_task_cancel", error as i32))?;
        drop(self.tasks[task.slot()].take());
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    pub(crate) fn cancel_all(&mut self) -> Result<(), Failure> {
        context()?;
        for slot in 0..CAPACITY {
            if self.tasks[slot].is_some() {
                let handle = self
                    .queue
                    .handle(TaskId(slot))
                    .map_err(|error| Failure::new("user_task_cancel_handle", error as i32))?;
                self.cancel(handle)?;
            }
        }
        Ok(())
    }

    fn finish(&mut self, id: TaskId) -> Result<(), Failure> {
        self.queue
            .finish_running()
            .map_err(|error| Failure::new("user_task_finish", error as i32))?;
        drop(self.tasks[id.0].take());
        self.queue
            .reap(id)
            .map_err(|error| Failure::new("user_task_reap", error as i32))
    }
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.kutox_user_dispatch_next")]
pub fn kutox_user_dispatch_next(
    dispatcher: &mut Dispatcher<'_, '_>,
    timer: &Timer<'_, '_>,
    quantum_us: u64,
) -> Result<Option<Event>, Failure> {
    context()?;
    let quantum = duration(quantum_us)?;
    let Some(id) = dispatcher
        .queue
        .dispatch()
        .map_err(|error| Failure::new("user_task_dispatch", error as i32))?
    else {
        return Ok(None);
    };
    let handle = dispatcher.queue.handle(id).expect("dispatched user task");
    let task = dispatcher.tasks[id.0]
        .as_mut()
        .expect("queued user task owner");
    let plan = task.budget.plan(quantum).expect("runnable user budget");
    let mut frame = task.context.restore_user_ctx();
    let mut entered = false;
    let result = user::run_context(
        &mut task.space,
        &mut frame,
        timer,
        plan,
        &mut task.counts,
        !task.started,
        &mut entered,
    );
    let (mut report, elapsed) = match result {
        Ok(result) => result,
        Err(error) => {
            if entered {
                dispatcher.finish(id)?;
            } else {
                dispatcher
                    .queue
                    .yield_running()
                    .map_err(|error| Failure::new("user_task_retry", error as i32))?;
            }
            return Err(error);
        }
    };
    task.started = true;
    task.budget.charge(elapsed);
    if task.budget.remaining() == 0 {
        report.stop = Stop::TimedOut;
    }
    if report.stop == Stop::QuantumExpired {
        task.context.save_user_ctx(&frame);
        dispatcher
            .queue
            .yield_running()
            .map_err(|error| Failure::new("user_task_yield", error as i32))?;
    } else {
        dispatcher.finish(id)?;
    }
    Ok(Some(Event {
        task: handle,
        report,
        elapsed_ticks: elapsed,
    }))
}

#[cfg(feature = "guest-tests")]
#[path = "guest_tests/user_tasks.rs"]
pub mod tests;
