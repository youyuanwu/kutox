//! Bounded, allocation-free guest debug controls, separate from acceptance modes.

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Selection {
    #[default]
    All,
    List,
    Filter(Filter),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Filter {
    bytes: [u8; 57],
    len: usize,
}

impl Filter {
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).expect("validated ASCII filter")
    }

    pub fn matches(&self, name: &str) -> bool {
        let pattern = &self.bytes[..self.len];
        let name = name.as_bytes();
        let (mut p, mut n, mut star, mut retry) = (0, 0, None, 0);
        while n < name.len() {
            if p < pattern.len() && (pattern[p] == b'?' || pattern[p] == name[n]) {
                p += 1;
                n += 1;
            } else if p < pattern.len() && pattern[p] == b'*' {
                star = Some(p);
                p += 1;
                retry = n;
            } else if let Some(previous) = star {
                retry += 1;
                n = retry;
                p = previous + 1;
            } else {
                return false;
            }
        }
        while p < pattern.len() && pattern[p] == b'*' {
            p += 1;
        }
        p == pattern.len()
    }
}

impl Selection {
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        if bytes == b"list" {
            return Some(Self::List);
        }
        let pattern = bytes.strip_prefix(b"filter=")?;
        if pattern.is_empty()
            || pattern.len() > 57
            || !pattern.iter().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_.*?".contains(byte)
            })
        {
            return None;
        }
        let mut bytes = [0; 57];
        bytes[..pattern.len()].copy_from_slice(pattern);
        Some(Self::Filter(Filter {
            bytes,
            len: pattern.len(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_rejects_invalid_and_overlength_input() {
        assert_eq!(Selection::parse(b"list"), Some(Selection::List));
        for value in [
            b"".as_slice(),
            b"filter=",
            b"filter=cpu.[ab]",
            b"filter=CPU.*",
            b"list\0",
        ] {
            assert_eq!(Selection::parse(value), None);
        }
        assert_eq!(Selection::parse(&[b'x'; 65]), None);
    }

    #[test]
    fn bounded_globs_match_exact_names_and_backtrack() {
        for (pattern, name, matches) in [
            ("cpu.*", "cpu.breakpoint_return", true),
            ("cpu.?rq*", "cpu.irq_lock_restore", true),
            ("*alignment*", "memory.alignment_reservations", true),
            ("cpu.*restore", "cpu.breakpoint_return", false),
            ("cpu", "cpu.irq_lock_restore", false),
            ("*", "", true),
            ("*a*b", "aaab", true),
            ("*a*b", "aaa", false),
        ] {
            let Selection::Filter(filter) =
                Selection::parse(std::format!("filter={pattern}").as_bytes()).unwrap()
            else {
                panic!("filter")
            };
            assert_eq!(filter.matches(name), matches);
        }
    }
}
