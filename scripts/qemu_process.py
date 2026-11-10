"""Owned QEMU process, socket and monitor lifecycle."""

import json
import os
from pathlib import Path
import re
import selectors
import signal
import socket
import struct
import subprocess
import tempfile
import time
from contextlib import nullcontext
import network_packets as ip_packets
import udp_packets
import dhcp_packets
from qemu_scenarios import *
from qemu_scenarios import SCENARIOS
from qemu_scenarios import DHCP_LIVE_CASES
from qemu_protocols import *
from qemu_validation import *


def execute_live(args, case, capture=None):
    deadline = time.monotonic() + args.timeout
    ready_at = None
    output, pending = bytearray(), bytearray()
    network_pending = bytearray()
    symbols = runtime_symbols(getattr(args, "nm", "nm"))
    host, peer = socket.socketpair()
    with host, peer:
        command = qemu_command(args, case, capture=capture, net_fd=peer.fileno())
        with subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                              stdin=subprocess.DEVNULL, pass_fds=(peer.fileno(),)) as process:
            peer.close()
            host.settimeout(min(args.timeout, 2))
            exchanges = getattr(args, "soak_exchanges", 1)
            protocol = dhcp_packets.LiveProtocol(host.sendall) if case in DHCP_LIVE_CASES else LiveIPProtocol(host.sendall, exchanges, 0.2 if exchanges > 1 else 0) if live_network(case) and LIVE_CASES[case][1] else None
            try:
                with selectors.DefaultSelector() as selector:
                    selector.register(process.stdout, selectors.EVENT_READ, "serial")
                    selector.register(host, selectors.EVENT_READ, "network")
                    while True:
                        now = time.monotonic()
                        if protocol is not None:
                            protocol.progress(now)
                        if process.poll() is not None:
                            raise AssertionError("persistent runtime exited before intentional host termination")
                        if now >= deadline:
                            raise subprocess.TimeoutExpired(command, args.timeout, output=bytes(output))
                        if ready_at is not None and now - ready_at >= getattr(args, "soak_seconds", 3):
                            validate_live_boot(case, output.decode("utf-8", errors="replace"), symbols)
                            elapsed = now - ready_at
                            # Only this owned child is terminated after measured sustained readiness.
                            process.terminate()
                            remaining_output, _ = process.communicate(timeout=2)
                            output.extend(remaining_output)
                            qemu_terminated = process.returncode == 0 and re.search(
                                rf"terminating on signal {signal.SIGTERM} from pid {os.getpid()}\b",
                                remaining_output.decode("utf-8", errors="replace"),
                            )
                            if process.returncode != -signal.SIGTERM and not qemu_terminated:
                                raise AssertionError("runtime exited independently of owned host termination")
                            host.settimeout(0.5)
                            if host.recv(1):
                                raise AssertionError("default runtime emitted late unexpected packet bytes")
                            if network_pending or protocol is not None and not protocol.complete:
                                raise AssertionError("incomplete live packet exchange")
                            break
                        for key, _ in selector.select(min(deadline - now, 0.1)):
                            chunk = os.read(key.fd, 4096)
                            if not chunk:
                                selector.unregister(key.fileobj)
                                continue
                            if key.data == "network":
                                if protocol is None:
                                    raise AssertionError("driver-only/unclaimed runtime emitted unexpected packet bytes")
                                network_pending.extend(chunk)
                                while len(network_pending) >= 4:
                                    length = struct.unpack_from("!I", network_pending)[0]
                                    if not 60 <= length <= 1514:
                                        raise AssertionError("invalid live Ethernet record")
                                    if len(network_pending) < 4 + length:
                                        break
                                    protocol.transmit(bytes(network_pending[4:4 + length]))
                                    del network_pending[:4 + length]
                                    if protocol.complete:
                                        ready_at = time.monotonic()
                                continue
                            output.extend(chunk)
                            pending.extend(chunk)
                            if len(output) > 1024 * 1024:
                                raise AssertionError("oversized default runtime journal")
                            while b"\n" in pending:
                                line, _, rest = pending.partition(b"\n")
                                pending = bytearray(rest)
                                decoded = line.decode("utf-8", errors="replace").rstrip("\r")
                                marker = (
                                    "NETWORK_STACK_READY" if protocol is not None
                                    else "NETWORK_STACK_WAIT state=source" if live_network(case)
                                    else "DRIVER_READY" if LIVE_CASES[case][1] else "DRIVER_RUNTIME"
                                )
                                if ready_at is None and (protocol is None or not protocol.started) and decoded.removeprefix("# ").startswith(marker):
                                    validate_live_boot(case, output.decode("utf-8", errors="replace"), symbols)
                                    if protocol is not None:
                                        protocol.start()
                                    else:
                                        ready_at = time.monotonic()
                            if len(pending) > 8192:
                                raise AssertionError("oversized unterminated runtime line")
                if len(output) > 1024 * 1024 or not output.endswith(b"\n") or elapsed < 3:
                    raise AssertionError("incomplete or insufficient measured runtime execution")
                validate_live_boot(case, output.decode("utf-8", errors="replace"), symbols)
            except AssertionError as error:
                raise AssertionError(f"{error}\n{output.decode('utf-8', errors='replace')}") from error
            except OSError as error:
                raise AssertionError(f"live peer I/O failed: {error}\n{output.decode('utf-8', errors='replace')}") from error
            finally:
                if process.poll() is None:
                    process.terminate()
                    try:
                        process.wait(timeout=2)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait()
        completed = subprocess.CompletedProcess(command, process.returncode, bytes(output))
        if case in DHCP_LIVE_CASES:
            completed.dhcp_packets = protocol.expected_packets
        return completed, elapsed


class QMP:
    def __init__(self, path):
        self.path = path
        self.socket = socket.socket(socket.AF_UNIX)
        self.stream = None

    def connect(self, process, deadline):
        while not self.path.exists():
            if process.poll() is not None or time.monotonic() >= deadline:
                raise AssertionError("owned QMP monitor did not become ready")
            time.sleep(0.01)
        self.socket.settimeout(2)
        self.socket.connect(str(self.path))
        self.stream = self.socket.makefile("rwb", buffering=0)
        if "QMP" not in json.loads(self.stream.readline(65536)):
            raise AssertionError("invalid owned QMP greeting")
        self.command("qmp_capabilities")

    def command(self, name, arguments=None):
        self.stream.write(json.dumps({"execute": name, "arguments": arguments or {}}).encode() + b"\n")
        while True:
            reply = json.loads(self.stream.readline(65536))
            if "error" in reply:
                raise AssertionError(f"owned QMP command failed: {reply['error']}")
            if "return" in reply:
                return

    def close(self):
        if self.stream is not None:
            self.stream.close()
        self.socket.close()


def execute_suite(args, case, capture):
    deadline = time.monotonic() + args.timeout
    output = bytearray()
    serial_pending = bytearray()
    network_pending = bytearray()
    transmitted = []
    kind = SCENARIOS[case].protocol
    network = kind == "network"
    hardening = kind == "managed-hardening"
    asynchronous = kind == "async-network"
    ip_network = kind == "ip"
    async_hardening = kind == "async-hardening"
    udp_network = kind == "udp"
    dhcp_network = kind == "dhcp"
    ambiguous = case.endswith("ambiguous")
    expected = async_hardening_packets() if async_hardening else ip_packets.replies() if ip_network else async_network_tx_packets() if asynchronous else [managed_hard_packet(sequence) for sequence in range(1, 5)] if hardening else [] if network and ambiguous else [network_packet(epoch, sequence) for epoch in (1, 2) for sequence in range(1, 7)] if network else expected_tx_packets()
    if udp_network:
        expected = udp_packets.replies()
    host, peer = socket.socketpair()
    with host, peer, (tempfile.TemporaryDirectory(prefix="kutox-hardening-") if async_hardening or dhcp_network else nullcontext(None)) as directory:
        command = qemu_command(args, case, capture, peer.fileno())
        monitor_path = Path(directory) / "qmp" if directory is not None else None
        if monitor_path is not None:
            command += ["-qmp", f"unix:{monitor_path},server=on,wait=off"]
        with subprocess.Popen(
            command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
            stdin=subprocess.DEVNULL, pass_fds=(peer.fileno(),),
        ) as process:
            peer.close()
            host.settimeout(min(args.timeout, 2))
            monitor = None
            try:
                if monitor_path is not None:
                    monitor = QMP(monitor_path)
                    monitor.connect(process, deadline)
                protocols = (dhcp_packets.Protocol(host.sendall, monitor),) if dhcp_network else (udp_packets.Protocol(host.sendall),) if udp_network else (AsyncHardProtocol(host.sendall, monitor),) if async_hardening else (IPProtocol(host.sendall),) if ip_network else (AsyncNetworkProtocol(host.sendall),) if asynchronous else (ManagedHardProtocol(host.sendall),) if hardening else (NetworkProtocol(host.sendall, ambiguous),) if network else (RXProtocol(host.sendall), HardProtocol(host.sendall))
                with selectors.DefaultSelector() as selector:
                    selector.register(process.stdout, selectors.EVENT_READ, "serial")
                    selector.register(host, selectors.EVENT_READ, "network")
                    while selector.get_map():
                        remaining = deadline - time.monotonic()
                        if remaining <= 0:
                            raise subprocess.TimeoutExpired(command, args.timeout, output=bytes(output))
                        for key, _ in selector.select(min(remaining, 0.1)):
                            chunk = os.read(key.fd, 4096)
                            if not chunk:
                                selector.unregister(key.fileobj)
                                continue
                            if key.data == "serial":
                                output.extend(chunk)
                                if len(output) > 4 * 1024 * 1024:
                                    raise AssertionError("oversized guest journal")
                                serial_pending.extend(chunk)
                                while b"\n" in serial_pending:
                                    line, _, rest = serial_pending.partition(b"\n")
                                    serial_pending = bytearray(rest)
                                    decoded = line.decode("utf-8", errors="replace").rstrip("\r")
                                    for protocol in protocols:
                                        protocol.observe(decoded)
                                if len(serial_pending) > 8192:
                                    raise AssertionError("oversized unterminated guest line")
                            else:
                                network_pending.extend(chunk)
                                while len(network_pending) >= 4:
                                    size = struct.unpack_from("!I", network_pending)[0]
                                    if not 60 <= size <= 1514:
                                        raise AssertionError("invalid guest TX socket record")
                                    if len(network_pending) < size + 4:
                                        break
                                    transmitted.append(bytes(network_pending[4:size + 4]))
                                    if ip_network or udp_network or dhcp_network:
                                        protocols[0].transmit(transmitted[-1])
                                    del network_pending[:size + 4]
                                    if len(transmitted) > (128 if dhcp_network else len(expected)):
                                        raise AssertionError("unexpected guest packet on isolated socket")
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        raise subprocess.TimeoutExpired(command, args.timeout, output=bytes(output))
                    returncode = process.wait(timeout=remaining)
                    if serial_pending or network_pending:
                        raise AssertionError("truncated guest serial/socket output")
                    if dhcp_network:
                        expected = protocols[0].expected_packets
                    if transmitted != expected:
                        raise AssertionError("isolated socket did not carry the exact Linux TX frames")
                    for protocol in protocols:
                        protocol.finish()
            except subprocess.TimeoutExpired as error:
                raise subprocess.TimeoutExpired(command, args.timeout, output=bytes(output)) from error
            except OSError as error:
                raise AssertionError(f"RX harness I/O failed: {error}\n{output.decode('utf-8', errors='replace')}") from error
            except AssertionError as error:
                raise AssertionError(f"{error}\n{output.decode('utf-8', errors='replace')}") from error
            finally:
                if monitor is not None:
                    monitor.close()
                if process.poll() is None:
                    process.terminate()
                    try:
                        process.wait(timeout=2)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait()
        completed = subprocess.CompletedProcess(command, returncode, bytes(output))
        if dhcp_network:
            completed.dhcp_packets = expected
        return completed

__all__ = ['execute_live', 'QMP', 'execute_suite']
