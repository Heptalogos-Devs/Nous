"""Native per-user service registration; never invokes elevation or a system daemon."""

from __future__ import annotations

import csv
import hashlib
import io
import os
import plistlib
import shutil
import subprocess
import sys
import tempfile
import xml.etree.ElementTree as ET
from dataclasses import dataclass
from pathlib import Path

from platformdirs import user_config_path
from pydantic import BaseModel

from nous.platform.errors import Unavailable

from . import initialize


@dataclass(frozen=True)
class ServiceStatus:
    manager: str
    label: str
    definition_path: Path
    installed: bool
    running: bool
    state: str
    last_exit_code: int | None = None


class _TaskStatus(BaseModel):
    installed: bool
    running: bool
    state: str
    last_exit_code: int | None = None


def _quote_systemd(value: str, *, environment: bool = True) -> str:
    escaped = value.replace("\\", "\\\\").replace('"', '\\"').replace("%", "%%")
    if environment:
        escaped = escaped.replace("$", "$$")
    return '"' + escaped + '"'


class UserService:
    def __init__(
        self,
        instance: Path,
        *,
        program: Path | None = None,
        host: str = "127.0.0.1",
        port: int = 7441,
    ):
        self.instance = instance.resolve()
        self.program = (
            program or Path(sys.executable).parent / ("nous.exe" if os.name == "nt" else "nous")
        ).resolve()
        self.host, self.port = host, port
        if not self.program.is_file():
            raise Unavailable("The installed nous executable was not found")
        if not 1 <= port <= 65535 or any(
            character in str(self.instance) + str(self.program) + host for character in "\n\r\x00"
        ):
            raise ValueError("Service paths, host and port must be valid")
        suffix = hashlib.sha256(str(self.instance).encode()).hexdigest()[:12]
        self.label = f"nous-wave-{suffix}"
        if sys.platform == "linux":
            self.manager = "systemd"
            self.path = user_config_path("systemd") / "user" / f"{self.label}.service"
        elif sys.platform == "darwin":
            self.manager = "launchd"
            self.path = Path.home() / "Library" / "LaunchAgents" / f"{self.label}.plist"
        elif os.name == "nt":
            self.manager = "task-scheduler"
            self.path = self.instance / "run" / "services" / f"{self.label}.xml"
        else:
            raise Unavailable("User services are unavailable on this operating system")

    @property
    def arguments(self) -> list[str]:
        return [
            str(self.program),
            "serve",
            "--instance",
            str(self.instance),
            "--host",
            self.host,
            "--port",
            str(self.port),
        ]

    def _run(
        self, arguments: list[str], *, allow_failure: bool = False, discard: bool = False
    ) -> subprocess.CompletedProcess[str]:
        try:
            result = subprocess.run(
                arguments,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL if discard else subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                encoding="utf-8",
                errors="replace",
                timeout=25,
                check=False,
            )
        except (FileNotFoundError, subprocess.TimeoutExpired) as error:
            raise Unavailable(
                "The user service manager is unavailable; use nous serve in the foreground"
            ) from error
        if result.returncode and not allow_failure:
            raise Unavailable(
                f"The user service manager rejected the operation: {result.stderr.strip()}"
            )
        return result

    def _available(self) -> None:
        if self.manager == "systemd":
            try:
                self._run(
                    ["systemctl", "--user", "show", "--property=Version", "--value"], discard=True
                )
            except Unavailable as error:
                raise Unavailable(
                    "The systemd user session is unavailable; run nous serve in the foreground"
                ) from error
        elif self.manager == "launchd":
            self._run(["launchctl", "print", f"gui/{os.getuid()}"], discard=True)
        elif shutil.which("schtasks") is None:
            raise Unavailable(
                "Windows Task Scheduler is unavailable; use nous serve in the foreground"
            )

    def definition(self, *, autostart: bool = False) -> bytes:
        if self.manager == "systemd":
            arguments = " ".join(_quote_systemd(value) for value in self.arguments)
            return (
                f"[Unit]\nDescription=Nous Wave user instance\nAfter=network-online.target\n\n"
                f"[Service]\nType=simple\nExecStart={arguments}\nWorkingDirectory={_quote_systemd(str(self.instance), environment=False)}\n"
                "Restart=on-failure\nRestartSec=5\nTimeoutStopSec=30\n\n[Install]\nWantedBy=default.target\n"
            ).encode()
        if self.manager == "launchd":
            return plistlib.dumps(
                {
                    "Label": self.label,
                    "ProgramArguments": self.arguments,
                    "WorkingDirectory": str(self.instance),
                    "RunAtLoad": autostart,
                    "KeepAlive": {"SuccessfulExit": False} if autostart else False,
                    "ThrottleInterval": 5,
                    "ExitTimeOut": 30,
                    "StandardOutPath": str(self.instance / "run" / "service.log"),
                    "StandardErrorPath": str(self.instance / "run" / "service-error.log"),
                }
            )
        identity = self._run(["whoami", "/user", "/fo", "csv", "/nh"]).stdout
        sid = next(csv.reader(io.StringIO(identity)))[1]
        namespace = "http://schemas.microsoft.com/windows/2004/02/mit/task"
        ET.register_namespace("", namespace)
        task = ET.Element(f"{{{namespace}}}Task", {"version": "1.4"})

        def node(
            parent: ET.Element, name: str, text: str | None = None, **attributes: str
        ) -> ET.Element:
            child = ET.SubElement(parent, f"{{{namespace}}}{name}", attributes)
            child.text = text
            return child

        if autostart:
            trigger = node(node(task, "Triggers"), "LogonTrigger")
            node(trigger, "Enabled", "true")
            node(trigger, "UserId", sid)
        principal = node(node(task, "Principals"), "Principal", id="Owner")
        node(principal, "UserId", sid)
        node(principal, "LogonType", "InteractiveToken")
        node(principal, "RunLevel", "LeastPrivilege")
        settings = node(task, "Settings")
        for key, value in (
            ("MultipleInstancesPolicy", "IgnoreNew"),
            ("DisallowStartIfOnBatteries", "false"),
            ("StopIfGoingOnBatteries", "false"),
            ("Enabled", "true"),
            ("ExecutionTimeLimit", "PT0S"),
        ):
            node(settings, key, value)
        action = node(node(task, "Actions", Context="Owner"), "Exec")
        node(action, "Command", str(self.program))
        node(action, "Arguments", subprocess.list2cmdline(self.arguments[1:]))
        node(action, "WorkingDirectory", str(self.instance))
        return ET.tostring(task, encoding="utf-16", xml_declaration=True)

    def install(self, *, autostart: bool = False) -> ServiceStatus:
        self._available()
        if self.path.exists():
            raise FileExistsError(
                "A service definition already exists; uninstall it before replacing its program or settings"
            )
        initialize(self.instance)
        data = self.definition(autostart=autostart)
        self.path.parent.mkdir(parents=True, exist_ok=True)
        with tempfile.NamedTemporaryFile(dir=self.path.parent, delete=False) as temporary:
            temporary.write(data)
            temporary.flush()
            os.fsync(temporary.fileno())
            staging = Path(temporary.name)
        try:
            os.link(staging, self.path)
        finally:
            staging.unlink(missing_ok=True)
        if self.manager == "systemd":
            self._run(["systemctl", "--user", "daemon-reload"])
            if autostart:
                self._run(["systemctl", "--user", "enable", self.label])
        elif self.manager == "task-scheduler":
            self._run(["schtasks", "/create", "/tn", self.label, "/xml", str(self.path)])
        return self.status()

    def status(self) -> ServiceStatus:
        self._available()
        if self.manager == "systemd":
            result = self._run(
                [
                    "systemctl",
                    "--user",
                    "show",
                    self.label + ".service",
                    "--property=LoadState,ActiveState,SubState,ExecMainStatus",
                ],
                allow_failure=True,
            )
            properties = dict(
                line.split("=", 1) for line in result.stdout.splitlines() if "=" in line
            )
            installed = properties.get("LoadState", "not-found") != "not-found"
            exit_code = properties.get("ExecMainStatus")
            return ServiceStatus(
                self.manager,
                self.label,
                self.path,
                installed,
                properties.get("ActiveState") == "active",
                properties.get("ActiveState", "not-installed")
                + "/"
                + properties.get("SubState", "unknown"),
                int(exit_code) if exit_code and exit_code.isdigit() else None,
            )
        if self.manager == "launchd":
            lines = self._run(["launchctl", "list"]).stdout.splitlines()
            for line in lines:
                fields = line.split()
                if len(fields) == 3 and fields[2] == self.label:
                    return ServiceStatus(
                        self.manager,
                        self.label,
                        self.path,
                        True,
                        fields[0] != "-",
                        "running" if fields[0] != "-" else "stopped",
                        int(fields[1]),
                    )
            return ServiceStatus(
                self.manager, self.label, self.path, self.path.exists(), False, "unloaded"
            )
        powershell = shutil.which("pwsh") or shutil.which("powershell")
        if powershell is None:
            raise Unavailable("PowerShell is required to inspect a per-user scheduled task")
        script = (
            "[Console]::OutputEncoding=[Text.UTF8Encoding]::new(); "
            f"$task=Get-ScheduledTask -TaskName '{self.label}' -ErrorAction SilentlyContinue; "
            "if($null -eq $task){@{installed=$false;running=$false;state='not-installed'}|ConvertTo-Json -Compress} "
            "else{$info=$task|Get-ScheduledTaskInfo;@{installed=$true;running=($task.State -eq 'Running');state=$task.State.ToString();last_exit_code=$info.LastTaskResult}|ConvertTo-Json -Compress}"
        )
        value = _TaskStatus.model_validate_json(
            self._run([powershell, "-NoProfile", "-NonInteractive", "-Command", script]).stdout
        )
        return ServiceStatus(
            self.manager,
            self.label,
            self.path,
            value.installed,
            value.running,
            value.state,
            value.last_exit_code,
        )

    def start(self) -> ServiceStatus:
        self._available()
        if not self.path.is_file():
            raise ValueError("Install this instance's user service before starting it")
        if self.manager == "systemd":
            self._run(["systemctl", "--user", "start", self.label])
        elif self.manager == "launchd":
            domain = f"gui/{os.getuid()}"
            if self.status().state == "unloaded":
                self._run(["launchctl", "bootstrap", domain, str(self.path)])
            self._run(["launchctl", "kickstart", f"{domain}/{self.label}"])
        else:
            self._run(["schtasks", "/run", "/tn", self.label])
        return self.status()

    def stop(self) -> ServiceStatus:
        status = self.status()
        if not status.running:
            return status
        if self.manager == "systemd":
            self._run(["systemctl", "--user", "stop", self.label])
        elif self.manager == "launchd":
            self._run(["launchctl", "bootout", f"gui/{os.getuid()}/{self.label}"])
        else:
            self._run(["schtasks", "/end", "/tn", self.label])
        return self.status()

    def uninstall(self) -> ServiceStatus:
        self._available()
        self.stop()
        if self.manager == "systemd":
            self._run(["systemctl", "--user", "disable", self.label], allow_failure=True)
        elif self.manager == "launchd":
            self._run(
                ["launchctl", "bootout", f"gui/{os.getuid()}/{self.label}"], allow_failure=True
            )
        elif self.manager == "task-scheduler":
            if self.status().installed:
                self._run(["schtasks", "/delete", "/tn", self.label, "/f"])
        self.path.unlink(missing_ok=True)
        if self.manager == "systemd":
            self._run(["systemctl", "--user", "daemon-reload"])
        return self.status()
