#!/usr/bin/env python3
"""Measure Ferramari's process tree, playback and Eco scenarios.

The harness starts an isolated instance.  Linux keeps the original private
D-Bus session; Windows uses an isolated APPDATA and the loopback control
endpoint.  It never edits the user's configuration or cache, and it does not
invent a zero for a metric the platform cannot measure: missing values are
``null`` in JSON and empty cells in CSV.

Examples::

    python scripts/benchmark.py --mode video --video C:/clips/1080p30.mp4 \
        --scenario steady --seconds 60 --repetitions 3 --json-out bench.json

    python scripts/benchmark.py --scenario parked --video C:/clips/1080p30.mp4 \
        --park-wait 300 --seconds 20

The old ``--mode web`` scenario is deliberately gone: the GTK-free build has no
web engine, so measuring it would only measure an error path.
"""

from __future__ import annotations

import argparse
import importlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any, Optional

from bench import common


PROFILE_ALIASES = {
    "quality": "performance",
    "performance": "performance",
    "balanced": "balanced",
    "eco": "power",
    "power": "power",
}


def platform_name(requested: str) -> str:
    if requested != "auto":
        return requested
    if sys.platform == "win32":
        return "windows"
    if sys.platform.startswith("linux"):
        return "linux"
    raise SystemExit("benchmark supports Linux and Windows only")


def default_binary() -> Path:
    suffix = ".exe" if os.name == "nt" else ""
    return Path("target") / "release" / f"ferramari{suffix}"


def default_ui_binary() -> Path:
    suffix = ".exe" if os.name == "nt" else ""
    return Path("target") / "release" / f"ferramari-ui{suffix}"


def parse_arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument("--platform", choices=["auto", "linux", "windows"], default="auto")
    parser.add_argument("--binary", type=Path, default=default_binary())
    parser.add_argument("--ui-binary", type=Path, default=default_ui_binary())
    parser.add_argument("--mode", choices=["null", "video", "stream"], default="video")
    parser.add_argument("--scenario", default="steady",
                        choices=["steady", "idle", "paused", "occluded", "parked",
                                 "resume", "recover", "switch", "ui", "churn",
                                 "long-run"])
    parser.add_argument("--video", type=Path)
    parser.add_argument("--video2", type=Path)
    parser.add_argument("--url", help="stream page URL for --mode stream")
    parser.add_argument("--profile", default="balanced",
                        choices=["quality", "performance", "balanced", "eco", "power"])
    parser.add_argument("--hwdec", default="auto", choices=["auto", "vaapi", "none"])
    parser.add_argument("--monitors", type=int, default=0,
                        help="require this many displays; 0 records whatever is present")
    parser.add_argument("--monitor", default="all",
                        help="monitor name for the wallpaper, or 'all'")
    parser.add_argument("--cover-monitor", type=int, default=0)
    parser.add_argument("--minimize-window", type=int, action="append", default=[],
                        help="temporarily minimize this PID's main window during "
                             "the run (useful when another window already covers "
                             "the wallpaper on a single-display desktop)")
    parser.add_argument("--page", default="home",
                        choices=["home", "library", "performance", "settings"])
    parser.add_argument("--ui-size", default="1180x780")
    parser.add_argument("--seconds", type=float, default=60.0)
    parser.add_argument("--pre-seconds", type=float, default=10.0)
    parser.add_argument("--warmup", type=float, default=5.0)
    parser.add_argument("--interval", type=float, default=1.0)
    parser.add_argument("--repetitions", type=int, default=1)
    parser.add_argument("--idle-timeout", type=int, default=30)
    parser.add_argument("--park-wait", type=float, default=300.0)
    parser.add_argument("--churn-cycles", type=int, default=10)
    parser.add_argument("--duration", type=float, default=0.0,
                        help="long-run duration in seconds; 86400 = 24 hours")
    parser.add_argument("--event-interval", type=float, default=3600.0)
    parser.add_argument("--idle-stub", type=Path,
                        help="Linux helper that owns Mutter.IdleMonitor on the private bus")
    parser.add_argument("--cover-helper", type=Path,
                        help="Linux cover-window helper (default: target/release/examples/cover_window)")
    parser.add_argument("--mute", dest="mute", action="store_true", default=True)
    parser.add_argument("--no-mute", dest="mute", action="store_false")
    parser.add_argument("--pss", dest="pss", action="store_true", default=True)
    parser.add_argument("--no-pss", dest="pss", action="store_false")
    parser.add_argument("--label", default="")
    parser.add_argument("--app-log", type=Path,
                        help="copy the isolated server log to this path")
    parser.add_argument("--json-out", type=Path)
    parser.add_argument("--jsonl-out", type=Path)
    parser.add_argument("--csv-out", type=Path)
    parser.add_argument("--keep-temp", action="store_true")
    return parser.parse_args()


def canonical_profile(value: str) -> str:
    return PROFILE_ALIASES.get(value, "balanced")


def write_config(path: Path, args: argparse.Namespace, scenario: str) -> None:
    idle_timeout = args.idle_timeout if scenario == "idle" else 86_400
    smart_pause = scenario in ("occluded", "parked", "resume", "churn", "long-run")
    config = {
        "version": 4,
        "mode": "MODE_NULL",
        "data_source": {"Default": ""},
        "playback_profile": canonical_profile(args.profile),
        "is_first_time": False,
        "is_static_wallpaper": False,
        "is_mute": args.mute,
        "audio_volume": 0 if args.mute else 50,
        "is_pause_when_maximized": smart_pause,
        "is_mute_when_maximized": False,
        "idle_timeout_sec": idle_timeout,
        "pause_on_battery": False,
        "hwdec": args.hwdec,
        "rotation_mode": "off",
        "rotation_interval_sec": 300,
        "theme": "dark",
        "language": "en",
    }
    common.write_json(path, config)


def build_environment(
    backend: Any, args: argparse.Namespace, root: Path
) -> Any:
    return backend.Environment(root)


def check_windows_foreign_instance() -> None:
    if sys.platform != "win32":
        return
    appdata = os.environ.get("APPDATA")
    if not appdata:
        return
    endpoint_path = Path(appdata) / "ferramari" / "control-endpoint.json"
    endpoint = common.read_json(endpoint_path)
    if not endpoint:
        return
    import socket
    try:
        with socket.create_connection(
            ("127.0.0.1", int(endpoint["port"])), timeout=2
        ) as stream:
            stream.sendall(
                (
                    json.dumps(
                        {
                            "token": endpoint.get("token"),
                            "method": "ping",
                            "params": None,
                        }
                    )
                    + "\n"
                ).encode("utf-8")
            )
            stream.settimeout(2)
            reply = stream.recv(4096)
        if reply.strip():
            raise SystemExit(
                "a Ferramari server is already running in this Windows session. "
                "The benchmark needs the session-wide instance mutex, so close "
                "the running server before measuring."
            )
    except OSError:
        pass


def apply_content(control: Any, args: argparse.Namespace) -> None:
    if args.mode == "null":
        return
    if args.mode == "video":
        if not args.video:
            raise SystemExit("--video is required in video mode")
        target = args.video.expanduser().resolve()
        if not target.is_file():
            raise SystemExit(f"video not found: {target}")
        control.command("video", str(target), args.monitor)
        return
    if not args.url:
        raise SystemExit("--url is required in stream mode")
    control.command("stream", args.url)


def first_frame(control: Any, timeout: float = 25.0) -> dict[str, Any]:
    def ready() -> Optional[dict[str, Any]]:
        snapshot = control.snapshot()
        playback = common.playback_from_snapshot(snapshot)
        if common.first_frame_reached(playback) and playback.get("active"):
            return playback
        return None

    try:
        return common.wait_for(ready, timeout, 0.1, "the first decoded frame")
    except TimeoutError:
        snapshot = control.snapshot()
        playback = common.playback_from_snapshot(snapshot)
        raise RuntimeError(
            "the player did not reach a first frame: "
            f"is_playing={playback.get('is_playing')} "
            f"is_decoding={playback.get('is_decoding')} "
            f"active={playback.get('active')}"
        )


def monitor_states(control: Any) -> dict[str, str]:
    return control.snapshot().get("monitor_states") or {}


def wait_monitor_state(control: Any, state: str, timeout: float = 10.0) -> str:
    def found() -> Optional[str]:
        for name, value in monitor_states(control).items():
            if value == state:
                return name
        return None

    name = common.wait_for(found, timeout, 0.1, f"a monitor in state {state!r}")
    return name


def monitor_count(snapshot: dict[str, Any]) -> int:
    resolutions = snapshot.get("monitor_resolutions") or {}
    if isinstance(resolutions, dict) and resolutions:
        return len(resolutions)
    states = snapshot.get("monitor_states") or {}
    return len(states) if isinstance(states, dict) else 0


class Runner:
    def __init__(self, args: argparse.Namespace, backend: Any, run_index: int):
        self.args = args
        self.backend = backend
        self.run_index = run_index
        self.root = Path(tempfile.mkdtemp(prefix="ferramari-bench-"))
        self.environment = build_environment(backend, args, self.root)
        self.control = self.environment.control()
        self.log_path = self.environment.log_path()
        self.server: Optional[subprocess.Popen[Any]] = None
        self.ui: Any = None
        self.cover: Any = None
        self.idle_stub: Optional[subprocess.Popen[Any]] = None
        self.samples: list[dict[str, Any]] = []
        self.run_result: dict[str, Any] = {"run": run_index + 1}
        self.minimized: list[int] = []
        self.started_at: Optional[float] = None

    def start(self) -> None:
        self.started_at = time.monotonic()
        if self.args.minimize_window and self.backend.__name__.endswith("windows"):
            for pid in self.args.minimize_window:
                if self.backend.set_window_minimized(pid, True):
                    self.minimized.append(pid)
            if self.minimized:
                time.sleep(0.8)
        write_config(self.environment.config_path(), self.args, self.args.scenario)
        if self.args.scenario == "idle" and self.args.idle_stub:
            self.idle_stub = subprocess.Popen(
                [str(self.args.idle_stub), "--idle-ms", "3600000"],
                env=self.environment.env(),
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            time.sleep(0.5)
        self.server = self.backend.start_server(
            self.args.binary, self.environment.env(), self.log_path
        )
        self.control.wait_ready(20)
        snapshot = self.control.snapshot()
        actual = monitor_count(snapshot)
        self.run_result["monitors"] = actual
        if self.args.monitors and actual != self.args.monitors:
            raise RuntimeError(
                f"this host reports {actual} monitor(s), the scenario requires "
                f"{self.args.monitors}"
            )
        if self.args.scenario in ("occluded", "parked", "resume", "long-run"):
            self.cover = self._make_cover()
        apply_content(self.control, self.args)
        if self.args.mode != "null":
            first_frame(self.control)
            if self.args.warmup > 0:
                time.sleep(self.args.warmup)

    def _make_cover(self) -> Any:
        if self.backend.__name__.endswith("windows"):
            script = Path(__file__).parent / "bench" / "cover_windows.ps1"
            return self.backend.CoverWindow(script, self.args.cover_monitor)
        helper = self.args.cover_helper or (
            Path("target") / "release" / "examples" / "cover_window"
        )
        return self.backend.CoverWindow(helper, self.args.cover_monitor)

    def close(self) -> None:
        if self.ui:
            try:
                self.ui.stop()
            except Exception:
                pass
        if self.cover:
            try:
                self.cover.stop()
            except Exception:
                pass
        if self.idle_stub and self.idle_stub.poll() is None:
            self.idle_stub.terminate()
        if self.control:
            self.control.quit()
        if self.server:
            try:
                self.server.wait(timeout=8)
            except subprocess.TimeoutExpired:
                if self.backend.__name__.endswith("windows"):
                    self.backend.kill_tree(self.server.pid)
                else:
                    try:
                        os.killpg(self.server.pid, 15)
                    except OSError:
                        self.server.kill()
                try:
                    self.server.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    self.server.kill()
            log = getattr(self.server, "_ferramari_log", None)
            if log:
                log.close()
        try:
            self.environment.close()
        except Exception:
            pass
        if self.minimized and self.backend.__name__.endswith("windows"):
            for pid in reversed(self.minimized):
                self.backend.set_window_minimized(pid, False)
            self.minimized.clear()
        if self.args.app_log and self.log_path.exists():
            self.args.app_log.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(self.log_path, self.args.app_log)
        if not self.args.keep_temp:
            shutil.rmtree(self.root, ignore_errors=True)

    def sample_phase(
        self,
        phase: str,
        duration: float,
        ui_pid: Optional[int] = None,
        stop_when: Optional[Any] = None,
    ) -> None:
        if duration <= 0:
            return
        previous = self._sample_processes(ui_pid)
        first = dict(previous)
        phase_start = time.monotonic()
        previous_at = phase_start
        deadline = previous_at + duration
        index = 0
        while time.monotonic() < deadline:
            if stop_when is not None and stop_when():
                break
            time.sleep(max(0.05, min(self.args.interval, deadline - time.monotonic())))
            now = time.monotonic()
            elapsed = now - previous_at
            current = self._sample_processes(ui_pid)
            snapshot = self.control.snapshot()
            playback = common.playback_from_snapshot(snapshot)
            gpu = self.backend_gpu.sample()
            groups = self._groups(current, ui_pid)
            metrics = common.group_metrics(
                previous,
                current,
                elapsed,
                groups,
                gpu,
                os.cpu_count() or 1,
            )
            processes = []
            for pid, info in sorted(current.items()):
                group = self._group_for(pid, groups, ui_pid)
                delta = None
                if pid in previous and info.cpu_seconds is not None:
                    old = previous[pid].cpu_seconds
                    if old is not None:
                        delta = (info.cpu_seconds - old) / elapsed * 100.0
                processes.append(
                    {
                        "pid": pid,
                        "ppid": info.ppid,
                        "name": info.name,
                        "group": group,
                        "cpu_pct": delta,
                        "rss_mib": info.rss_bytes / common.MIB
                        if info.rss_bytes is not None
                        else None,
                        "pss_mib": info.pss_bytes / common.MIB
                        if info.pss_bytes is not None
                        else None,
                        "threads": info.threads,
                        "gpu_luid": gpu.per_pid_luid.get(pid),
                    }
                )
            sample = {
                "phase": phase,
                "sample": index,
                "t": now - phase_start,
                "wall_t": now - (self.started_at or phase_start),
                "groups": metrics,
                "playback": playback,
                "gpu_global_pct": gpu.global_percent,
                "vram_global_mib": gpu.global_vram_bytes / common.MIB
                if gpu.global_vram_bytes is not None
                else None,
                "mpv_pids": sorted(
                    pid
                    for pid, info in current.items()
                    if info.name.lower() in ("mpv", "mpv.exe")
                ),
                "processes": processes,
                "notes": [gpu.reason] if gpu.reason else [],
                "ui_pid": ui_pid,
            }
            self.samples.append(sample)
            previous = current
            previous_at = now
            index += 1
        measured = time.monotonic() - phase_start
        if first and previous and measured > 0:
            groups = self._groups(previous, ui_pid)
            phase_cpu: dict[str, Optional[float]] = {}
            for group, pids in groups.items():
                deltas = [
                    previous[pid].cpu_seconds - first[pid].cpu_seconds
                    for pid in pids
                    if pid in first
                    and pid in previous
                    and first[pid].cpu_seconds is not None
                    and previous[pid].cpu_seconds is not None
                ]
                phase_cpu[group] = (
                    sum(deltas) / measured * 100.0 if deltas else None
                )
            self.run_result.setdefault("phase_cpu", {})[phase] = phase_cpu

    def _sample_processes(self, ui_pid: Optional[int]) -> dict[int, Any]:
        tree = self.backend_procs.sample_tree([self.server.pid])  # type: ignore[union-attr]
        if ui_pid:
            tree.update(
                self.backend_procs.sample_tree([ui_pid], include_descendants=True)  # type: ignore[union-attr]
            )
        return tree

    @staticmethod
    def _group_for(pid: int, groups: dict[str, set[int]], ui_pid: Optional[int]) -> str:
        for name, pids in groups.items():
            if pid in pids:
                return name
        return "other"

    def _groups(self, current: dict[int, Any], ui_pid: Optional[int]) -> dict[str, set[int]]:
        server = {self.server.pid} & set(current)
        mpv = {
            pid
            for pid, info in current.items()
            if info.name.lower() in ("mpv", "mpv.exe")
        }
        # Server descendants that are neither the server nor mpv.
        server_tree = set()
        # The sampler already restricted the tree to the server; derive the
        # descendant set from parent links present in `current`.
        if self.server:
            children: dict[int, list[int]] = {}
            for pid, info in current.items():
                children.setdefault(info.ppid, []).append(pid)
            stack = [self.server.pid]
            while stack:
                pid = stack.pop()
                if pid in server_tree:
                    continue
                server_tree.add(pid)
                stack.extend(children.get(pid, []))
        children = server_tree - server - mpv
        groups = {
            "ferramari": server,
            "mpv": mpv,
            "children": children,
            "total": server_tree,
        }
        if ui_pid:
            ui_tree = {ui_pid} & set(current)
            groups["ui"] = ui_tree
        return groups

    @property
    def backend_procs(self) -> Any:
        return self._procs

    @backend_procs.setter
    def backend_procs(self, value: Any) -> None:
        self._procs = value

    @property
    def backend_gpu(self) -> Any:
        return self._gpu

    @backend_gpu.setter
    def backend_gpu(self, value: Any) -> None:
        self._gpu = value


def run_scenario(
    args: argparse.Namespace, backend: Any, run_index: int
) -> tuple[dict[str, Any], list[dict[str, Any]]]:
    runner = Runner(args, backend, run_index)
    if backend.__name__.endswith("linux"):
        runner.backend_procs = backend.Procs(with_pss=args.pss)
    else:
        runner.backend_procs = backend.Procs()
    runner.backend_gpu = backend.Gpu()
    try:
        runner.start()
        scenario = args.scenario
        if scenario == "steady":
            runner.sample_phase("steady", args.seconds)
        elif scenario == "paused":
            runner.control.command("toggle_pause")
            common.wait_for(
                lambda: common.playback_from_snapshot(
                    runner.control.snapshot()
                ).get("is_paused_by_user"),
                10,
                0.1,
                "the user pause",
            )
            time.sleep(1.0)
            runner.sample_phase("paused", args.seconds)
        elif scenario == "idle":
            if args.idle_stub and hasattr(backend, "LinuxEnvironment"):
                time.sleep(1.0)
            name = wait_monitor_state(
                runner.control, "idle", timeout=max(90.0, args.idle_timeout + 75)
            )
            runner.run_result["idle_monitor"] = name
            runner.sample_phase("idle", args.seconds)
        elif scenario in ("occluded", "resume"):
            runner.cover.start()
            wait_monitor_state(runner.control, "occluded", timeout=10)
            if scenario == "occluded":
                runner.sample_phase("occluded", args.seconds)
            else:
                time.sleep(min(2.0, args.warmup))
            runner.cover.stop()
            wait_monitor_state(runner.control, "active", timeout=10)
            first_frame(runner.control)
            runner.sample_phase("resumed", args.seconds)
        elif scenario == "parked":
            runner.cover.start()
            wait_monitor_state(runner.control, "occluded", timeout=10)
            runner.sample_phase("occluded", args.pre_seconds)
            tail = common.LogTail(runner.log_path)
            marker = tail.wait_marker(
                "parked the decoder", timeout=args.park_wait + 90
            )
            if marker is None:
                raise RuntimeError(
                    "the decoder did not park within the expected window; "
                    "see the server log for the Eco transitions"
                )
            runner.run_result["parked_log"] = marker
            runner.sample_phase("parked", args.seconds)
            runner.cover.stop()
            wait_monitor_state(runner.control, "active", timeout=10)
            first_frame(runner.control)
            runner.sample_phase("resumed", args.seconds)
        elif scenario == "recover":
            runner.sample_phase("before", max(5.0, args.pre_seconds))
            current = runner._sample_processes(None)
            mpv = sorted(
                pid
                for pid, info in current.items()
                if info.name.lower() in ("mpv", "mpv.exe")
            )
            if not mpv:
                raise RuntimeError("no mpv process found to kill")
            victim = mpv[0]
            killed_at = time.monotonic()
            if backend.__name__.endswith("windows"):
                subprocess.run(
                    ["taskkill", "/PID", str(victim), "/F"],
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                    timeout=10,
                    creationflags=0x08000000,
                )
            else:
                os.kill(victim, 9)
            new_pid_ms = None
            first_frame_ms = None
            deadline = killed_at + 30
            while time.monotonic() < deadline:
                current = runner._sample_processes(None)
                new_mpv = [
                    pid
                    for pid, info in current.items()
                    if info.name.lower() in ("mpv", "mpv.exe") and pid != victim
                ]
                if new_mpv and new_pid_ms is None:
                    new_pid_ms = (time.monotonic() - killed_at) * 1000
                playback = common.playback_from_snapshot(runner.control.snapshot())
                if new_mpv and common.first_frame_reached(playback) and playback.get("active"):
                    first_frame_ms = (time.monotonic() - killed_at) * 1000
                    break
                time.sleep(0.1)
            runner.run_result["recovery_new_pid_ms"] = new_pid_ms
            runner.run_result["recovery_first_frame_ms"] = first_frame_ms
            if new_pid_ms is None:
                raise RuntimeError("mpv did not restart after the kill")
            runner.sample_phase("after", args.seconds)
        elif scenario == "switch":
            if not args.video2:
                raise SystemExit("--video2 is required for the switch scenario")
            current = runner._sample_processes(None)
            before_pids = sorted(
                pid
                for pid, info in current.items()
                if info.name.lower() in ("mpv", "mpv.exe")
            )
            target = args.video2.expanduser().resolve()
            if not target.is_file():
                raise SystemExit(f"video not found: {target}")
            tail = common.LogTail(runner.log_path)
            tail.read()  # discard the initial load
            command_at = time.monotonic()
            runner.control.command("video", str(target), args.monitor)
            # The player reuses the mpv process and the loadfile reply is
            # asynchronous. Waiting for `first_frame` immediately would read
            # the old file's still-active stats. Confirm the reuse in the log,
            # then wait for the fresh decoder probe and a first frame.
            reuse_line = tail.wait_marker("[Mode]", 6)
            runner.run_result["switch_reused_ms"] = (
                time.monotonic() - command_at
            ) * 1000
            if reuse_line is None:
                raise RuntimeError("the server did not confirm the video switch")
            decoder_line = tail.wait_marker("[HwDec] requested", 20)
            if decoder_line is None:
                raise RuntimeError("the switched file did not report a decoder")
            first_frame(runner.control)
            runner.run_result["switch_first_frame_ms"] = (
                time.monotonic() - command_at
            ) * 1000
            runner.run_result["switch_log"] = {
                "reuse": reuse_line,
                "decoder": decoder_line,
            }
            after = runner._sample_processes(None)
            after_pids = sorted(
                pid
                for pid, info in after.items()
                if info.name.lower() in ("mpv", "mpv.exe")
            )
            runner.run_result["mpv_pid_reused"] = before_pids == after_pids
            runner.sample_phase("switched", args.seconds)
        elif scenario == "churn":
            pid = args.minimize_window[0]
            wait_monitor_state(runner.control, "active", timeout=5)
            tail = common.LogTail(runner.log_path)
            tail.read()  # discard startup transitions
            first = runner.backend_procs.sample_tree([runner.server.pid])
            started = time.monotonic()
            for cycle in range(args.churn_cycles):
                # One second up and one second down is longer than the 750 ms
                # window sampler period and shorter than the two-second
                # occlusion confirmation. Before the hysteresis every cycle
                # produced a pause and a resume; after it, none should.
                backend.set_window_minimized(pid, False)
                time.sleep(1.0)
                backend.set_window_minimized(pid, True)
                time.sleep(1.0)
            elapsed = time.monotonic() - started
            last = runner.backend_procs.sample_tree([runner.server.pid])
            server_cpu = None
            if (
                runner.server.pid in first
                and runner.server.pid in last
                and first[runner.server.pid].cpu_seconds is not None
                and last[runner.server.pid].cpu_seconds is not None
                and elapsed > 0
            ):
                server_cpu = (
                    last[runner.server.pid].cpu_seconds
                    - first[runner.server.pid].cpu_seconds
                ) / elapsed * 100.0
            lines = tail.read()
            eco = [line for line in lines if "[Eco]" in line]
            runner.run_result["churn"] = {
                "cycles": args.churn_cycles,
                "elapsed_s": elapsed,
                "server_cpu_pct": server_cpu,
                "eco_lines": len(eco),
                "eco_samples": eco[:12],
            }
            wait_monitor_state(runner.control, "active", timeout=6)
            runner.sample_phase("after_churn", max(5.0, args.pre_seconds))
        elif scenario == "ui":
            runner.ui = backend.UiProcess(
                args.ui_binary, runner.environment.env(), args.page, args.ui_size
            )
            runner.ui.start()
            runner.sample_phase(f"ui_{args.page}_visible", args.seconds, runner.ui.process.pid)
            runner.ui.hide()
            runner.sample_phase("ui_hidden", args.seconds, runner.ui.process.pid)
        elif scenario == "long-run":
            duration = args.duration or 86400.0
            deadline = time.monotonic() + duration
            next_event = time.monotonic() + args.event_interval
            event_index = 0
            soak_log = common.LogTail(runner.log_path)
            soak_log.read()
            soak_events: list[dict[str, Any]] = []
            while time.monotonic() < deadline:
                remaining = deadline - time.monotonic()
                runner.sample_phase(
                    "long-run", max(0.0, min(args.interval * 10, 30.0, remaining))
                )
                now = time.monotonic()
                if now >= deadline:
                    break
                if now >= next_event:
                    event_index += 1
                    event_at = now - (runner.started_at or now)
                    if event_index % 3 == 1:
                        # kill one player and wait for supervision to recover it
                        current = runner._sample_processes(None)
                        mpv = sorted(
                            pid
                            for pid, info in current.items()
                            if info.name.lower() in ("mpv", "mpv.exe")
                        )
                        event: dict[str, Any] = {"kind": "kill", "at_s": event_at}
                        if mpv:
                            victim = mpv[0]
                            event["pid"] = victim
                            killed_at = time.monotonic()
                            if backend.__name__.endswith("windows"):
                                subprocess.run(
                                    ["taskkill", "/PID", str(victim), "/F"],
                                    stdout=subprocess.DEVNULL,
                                    stderr=subprocess.DEVNULL,
                                    timeout=10,
                                    creationflags=0x08000000,
                                )
                            else:
                                os.kill(victim, 9)
                            new_pid_ms = None
                            first_frame_ms = None
                            recovery_deadline = killed_at + 20
                            while time.monotonic() < recovery_deadline:
                                current = runner._sample_processes(None)
                                new_mpv = [
                                    pid
                                    for pid, info in current.items()
                                    if info.name.lower() in ("mpv", "mpv.exe")
                                    and pid != victim
                                ]
                                if new_mpv and new_pid_ms is None:
                                    new_pid_ms = (time.monotonic() - killed_at) * 1000
                                if new_mpv:
                                    playback = common.playback_from_snapshot(
                                        runner.control.snapshot()
                                    )
                                    if common.first_frame_reached(playback) and playback.get(
                                        "active"
                                    ):
                                        first_frame_ms = (
                                            time.monotonic() - killed_at
                                        ) * 1000
                                        break
                                time.sleep(0.2)
                            event["new_pid_ms"] = new_pid_ms
                            event["first_frame_ms"] = first_frame_ms
                            if new_pid_ms is None:
                                soak_events.append(event)
                                soak_events[-1]["error"] = "no replacement mpv"
                                runner.run_result["soak_events"] = soak_events
                                raise RuntimeError(
                                    "long-run: mpv did not restart after a scheduled kill"
                                )
                        soak_events.append(event)
                    elif event_index % 3 == 2 and runner.cover:
                        remaining = deadline - time.monotonic()
                        if remaining < 65.0:
                            soak_events.append(
                                {
                                    "kind": "cover_skipped",
                                    "at_s": event_at,
                                    "reason": "not enough time left for a 60 s cover",
                                }
                            )
                        else:
                            event = {"kind": "cover", "at_s": event_at}
                            runner.cover.start()
                            hold = min(60.0, remaining - 5.0)
                            time.sleep(hold)
                            runner.cover.stop()
                            event["duration_s"] = hold
                            soak_events.append(event)
                    else:
                        runner.control.command("reload")
                        soak_events.append({"kind": "reload", "at_s": event_at})
                    next_event = time.monotonic() + args.event_interval
            runner.run_result["soak_events"] = soak_events
            runner.run_result["soak"] = common.soak_summary(
                runner.samples, soak_log.read(), duration
            )
        else:
            raise SystemExit(f"unknown scenario {scenario}")
        runner.run_result["phases"] = common.summarize_samples(runner.samples)
        if runner.samples:
            runner.run_result["processes"] = runner.samples[-1].get("processes") or []
            runner.run_result["last_snapshot"] = {
                "monitor_states": runner.samples[-1]
                .get("playback", {})
                .get("monitor_states"),
                "mpv_pids": runner.samples[-1].get("mpv_pids"),
            }
        return runner.run_result, runner.samples
    finally:
        runner.close()


def combine(results: list[dict[str, Any]], samples: list[dict[str, Any]]) -> dict[str, Any]:
    phases: dict[str, list[dict[str, Any]]] = {}
    for sample in samples:
        phases.setdefault(sample["phase"], []).append(sample)
    combined: dict[str, Any] = {}
    for phase, phase_samples in phases.items():
        combined[phase] = common.summarize_samples(
            [{**sample, "phase": phase} for sample in phase_samples]
        )[phase]
    phase_cpu: dict[str, dict[str, Any]] = {}
    for result in results:
        for phase, groups in (result.get("phase_cpu") or {}).items():
            bucket = phase_cpu.setdefault(phase, {}).setdefault(
                "__values__", {}  # type: ignore[arg-type]
            )
            for group, value in groups.items():
                bucket.setdefault(group, []).append(value)
    phase_cpu_summary = {
        phase: {
            group: common.summarize(values)
            for group, values in data["__values__"].items()
        }
        for phase, data in phase_cpu.items()
    }
    return {
        "runs": results,
        "combined": combined,
        "phase_cpu": phase_cpu_summary,
    }


def main() -> int:
    args = parse_arguments()
    platform = platform_name(args.platform)
    backend = importlib.import_module(f"bench.{platform}")
    if platform == "windows":
        check_windows_foreign_instance()
    args.binary = args.binary.resolve()
    args.ui_binary = args.ui_binary.resolve()
    if not args.binary.is_file():
        raise SystemExit(f"binary not found: {args.binary}")
    if platform == "windows" and args.hwdec == "vaapi":
        raise SystemExit("VA-API is not available on Windows")
    if args.mode == "video" and not args.video:
        raise SystemExit("--video is required in video mode")
    if args.scenario in ("idle", "paused", "occluded", "parked", "resume", "recover", "switch", "churn") and args.mode == "null":
        raise SystemExit(f"--scenario {args.scenario} needs a wallpaper; use --mode video or stream")
    if args.scenario == "churn" and not args.minimize_window:
        raise SystemExit("--scenario churn needs --minimize-window PID (a window that can cover the wallpaper)")
    displays = backend.list_displays()
    environment = common.gather_environment(
        args.binary,
        f"{platform}",
        displays,
        extra={
            "label": args.label,
            "profile": canonical_profile(args.profile),
            "scenario": args.scenario,
            "mode": args.mode,
            "mute": args.mute,
            "python_harness_schema": common.SCHEMA_VERSION,
        },
    )
    all_samples: list[dict[str, Any]] = []
    results: list[dict[str, Any]] = []
    started = time.time()
    for index in range(max(1, args.repetitions)):
        print(f"run {index + 1}/{max(1, args.repetitions)}: {args.scenario} ...")
        result, samples = run_scenario(args, backend, index)
        results.append(result)
        all_samples.extend(samples)
        common.print_summary(result["phases"], environment)
        for phase, groups in (result.get("phase_cpu") or {}).items():
            values = " ".join(
                f"{group}={value:.2f}%"
                for group, value in groups.items()
                if value is not None
            )
            print(f"  phase_cpu[{phase}] {values}")
    payload = {
        "schema": common.SCHEMA_VERSION,
        "generated_at": common.utc_now(),
        "environment": environment,
        "configuration": {
            "scenario": args.scenario,
            "mode": args.mode,
            "profile": canonical_profile(args.profile),
            "hwdec": args.hwdec,
            "video": str(args.video) if args.video else None,
            "video_sha256": common.sha256_file(args.video) if args.video else None,
            "video2": str(args.video2) if args.video2 else None,
            "seconds": args.seconds,
            "warmup": args.warmup,
            "interval": args.interval,
            "idle_timeout": args.idle_timeout,
            "park_wait": args.park_wait,
            "duration": args.duration,
            "label": args.label,
        },
        **combine(results, all_samples),
        "missing": {
            "gpu": "read from sysfs on AMDGPU, nvidia-smi on NVIDIA, PDH on Windows; "
                   "null when the platform exposes no counter",
            "vram": "same sources as gpu; per-process VRAM is Windows-only",
            "pss": "Linux smaps_rollup; Windows reports working set, not PSS",
            "fps_dropped": "primary player only; non-primary mpv stats are not "
                           "probed every second to avoid multiplied IPC traffic",
            "monitors_2_3": "requires a host with two or three physical displays",
        },
        "elapsed_s": time.time() - started,
    }
    if args.json_out:
        common.write_json(args.json_out, payload)
        print(f"json: {args.json_out}")
    if args.jsonl_out:
        args.jsonl_out.parent.mkdir(parents=True, exist_ok=True)
        with args.jsonl_out.open("w", encoding="utf-8") as handle:
            for sample in all_samples:
                handle.write(json.dumps(sample, ensure_ascii=False) + "\n")
        print(f"jsonl: {args.jsonl_out}")
    if args.csv_out:
        rows = [
            common.sample_to_row(sample["phase"], sample["sample"], sample)
            for sample in all_samples
        ]
        common.write_csv(args.csv_out, rows)
        print(f"csv: {args.csv_out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
