#!/usr/bin/env python3
"""Checks `burdin dap`, the Debug Adapter Protocol server (`spec/debug.md`, DBG11): it forwards to a backend and
adds Burdin behavior at four points. The backend is a Python stub (`BURDIN_DAP_BACKEND`), so the check needs no
debugger; the same augmentations are unit-tested against a scripted peer in `crates/burdin_dap/tests/protocol.rs`.

BURDIN selects the driver, like tests/run_tests.py; otherwise target/{debug,release}/burdin is used.

What it checks:
  * the `initialize` response advertises the `panic` exception filter next to the backend's own filters;
  * `setExceptionBreakpoints` with that filter and `setFunctionBreakpoints` reach the backend as one merged list —
    the client's breakpoints plus `burdin_panic_begin` — and the client still gets an answer to each;
  * a step that stops in generated code is stepped out of and never reported, and the next stop is;
  * a `launch` request naming a `.bd` `target` builds it with `-g`, forwards the executable and the lldb
    formatters' import, and answers success; with `test`, the test's index becomes the program's first argument;
  * an unknown argument is a usage error.
"""

import json
import os
import sys
import time

import harness

problems = []
checks = 0
skips = []


def add_problem(message: str) -> None:
    problems.append(message)


def skip(reason: str) -> None:
    skips.append(reason)


burdin = harness.require_burdin()
windows_host = harness.PLATFORM == "windows"

# The stub backend: answers DAP requests the way lldb-dap would, sends the two stops the glue-skipping check needs,
# and logs every message it receives. It exits after the `disconnect` request, which ends the adapter's session.
STUB = '''\
import json
import os
import sys

log = open(os.environ["BURDIN_DAP_LOG"], "w", encoding="utf-8")
stack_traces = 0


def read():
    headers = {}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.decode("utf-8").strip()
        if not line:
            break
        name, value = line.split(":", 1)
        headers[name.strip().lower()] = value.strip()
    body = sys.stdin.buffer.read(int(headers["content-length"]))
    return json.loads(body)


def write(message):
    body = json.dumps(message).encode("utf-8")
    sys.stdout.buffer.write(b"Content-Length: %d\\r\\n\\r\\n" % len(body) + body)
    sys.stdout.buffer.flush()


def response(request, body=None):
    write({"seq": 1, "type": "response", "request_seq": request["seq"], "success": True,
           "command": request["command"], "body": body or {}})


def event(name, body):
    write({"seq": 1, "type": "event", "event": name, "body": body})


def frame(name, path):
    return {"id": 1, "name": name, "line": 1, "column": 1, "source": {"path": path}}


while True:
    message = read()
    if message is None:
        break
    log.write(json.dumps(message) + "\\n")
    log.flush()
    command = message.get("command")
    if command == "initialize":
        response(message, {"exceptionBreakpointFilters": [{"filter": "stackFrame", "label": "Stack frame"}]})
    elif command == "stackTrace":
        stack_traces += 1
        # The first step lands in drop glue, the second in the user's own function.
        frames = [frame("burdin.drop.app.Item", "/src/app.bd")] if stack_traces == 1 else [frame("app.load", "/src/app.bd")]
        response(message, {"stackFrames": frames, "totalFrames": 1})
    elif command == "configurationDone":
        response(message)
        event("stopped", {"reason": "step", "threadId": 1})
    elif command == "stepOut":
        response(message)
        event("stopped", {"reason": "step", "threadId": 1})
    elif command == "disconnect":
        response(message)
        break
    else:
        response(message)
log.close()
'''




class Client:
    """`burdin dap` as an editor drives it: one request at a time, waiting for the stop each request produces."""

    def __init__(self, proc):
        self.proc = proc
        self.seq = 0
        self.pending = b""

    def send(self, command, arguments=None):
        self.seq += 1
        message = {"seq": self.seq, "type": "request", "id": self.seq, "command": command, "arguments": arguments or {}}
        body = json.dumps(message).encode("utf-8")
        self.proc.stdin.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
        self.proc.stdin.flush()

    def read(self, timeout=60):
        """The next message from the adapter, or `None` when it closed its output."""
        import select

        while True:
            if b"\r\n\r\n" in self.pending:
                header, _, rest = self.pending.partition(b"\r\n\r\n")
                length = int(header.split(b":")[1].strip())
                if len(rest) >= length:
                    self.pending = rest[length:]
                    return json.loads(rest[:length])
            ready, _, _ = select.select([self.proc.stdout], [], [], timeout)
            if not ready:
                raise TimeoutError("the adapter sent nothing")
            chunk = os.read(self.proc.stdout.fileno(), 65536)
            if not chunk:
                return None
            self.pending += chunk

    def wait_for(self, predicate, timeout=60):
        while True:
            message = self.read(timeout)
            if message is None:
                raise EOFError("the adapter closed its output")
            if predicate(message):
                return message


def start_session(work, wrapper, log):
    """Starts `burdin dap` with the stub backend and returns the client and its process."""
    import subprocess

    if log.exists():
        log.unlink()
    env = dict(os.environ, BURDIN_DAP_BACKEND=str(wrapper), BURDIN_DAP_LOG=str(log))
    proc = subprocess.Popen(
        [burdin, "dap"], cwd=str(work), env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE
    )
    return Client(proc), proc


def backend_log(log):
    """The messages the stub received, in order."""
    messages = []
    for line in harness.read_text(log).splitlines():
        try:
            messages.append(json.loads(line))
        except ValueError:
            pass  # a line the stub is still writing
    return messages


def last_launch(log):
    return next((m.get("arguments", {}) for m in backend_log(log) if m.get("command") in ("launch", "attach")), {})


with harness.temporary_dir("burdin-dap-") as work:
    stub = work / "stub_backend.py"
    harness.write_text(stub, STUB)
    log = work / "backend.log"
    target = work / "dapapp.bd"
    harness.write_text(
        target,
        "fn load(): i32 {\n    let values = [1, 2, 3]\n    values[0]\n}\n\n"
        'test "loads" {\n    assert(load() == 1)\n}\n\n'
        'test "adds" {\n    assert(1 + 1 == 2)\n}\n\n'
        'fn main() {\n    println(load().to_string())\n}\n',
    )

    # The backend program is Python run with the stub as its argument: a small wrapper script names both.
    wrapper = work / "stub-backend"
    harness.write_text(wrapper, '#!/bin/sh\nexec "{}" "{}"\n'.format(sys.executable, stub))
    os.chmod(wrapper, 0o755)

    # An unknown argument is a usage error, not a protocol message, on every platform.
    checks += 1
    bad = harness.run([burdin, "dap", "--bogus"], cwd=work)
    if bad.returncode != 2 or "unknown argument" not in harness.text(bad.stderr):
        add_problem(f"`burdin dap --bogus` was not a usage error: {harness.text(bad.stderr)}")

    if windows_host:
        skip("`burdin dap`'s protocol was not checked: the stub backend is a shell script")
    else:
        client, proc = start_session(work, wrapper, log)
        try:
            client.send("initialize", {"clientID": "check"})
            initialize = client.wait_for(lambda m: m.get("type") == "response" and m.get("command") == "initialize")
            checks += 1
            filters = [f.get("filter") for f in initialize.get("body", {}).get("exceptionBreakpointFilters", [])]
            if "stackFrame" not in filters or "panic" not in filters:
                add_problem(f"the initialize response does not advertise the panic filter: {initialize}")

            client.send("launch", {"target": str(target)})
            launch = client.wait_for(lambda m: m.get("type") == "response" and m.get("command") == "launch")
            checks += 1
            if not launch.get("success"):
                add_problem(f"the launch of a `target` failed: {launch}")
            arguments = last_launch(log)
            checks += 1
            if "dapapp" not in arguments.get("program", ""):
                add_problem(f"the launch request does not name the built executable: {arguments}")
            pre_run = " ".join(arguments.get("preRunCommands", []))
            if "command script import" not in pre_run or "burdin_formatters.py" not in pre_run:
                add_problem(f"the launch request imports no lldb formatters: {pre_run}")
            checks += 1
            if "target" in arguments or "test" in arguments:
                add_problem(f"the adapter forwards its own fields: {sorted(arguments)}")

            client.send("setExceptionBreakpoints", {"filters": ["panic"]})
            client.send("setFunctionBreakpoints", {"breakpoints": [{"name": "main"}]})
            for command in ("setExceptionBreakpoints", "setFunctionBreakpoints"):
                response = client.wait_for(lambda m, c=command: m.get("type") == "response" and m.get("command") == c)
                checks += 1
                if not response.get("success"):
                    add_problem(f"the client was not answered for `{command}`: {response}")

            # The adapter answers the client as soon as it has written to the backend, and the stub logs a request
            # only when it has read it, so wait for the merged list to arrive instead of reading the log at once.
            deadline = time.monotonic() + 30
            while True:
                merged = [m for m in backend_log(log) if m.get("command") == "setFunctionBreakpoints"]
                names = [b.get("name") for b in merged[-1]["arguments"]["breakpoints"]] if merged else []
                if names == ["main", "burdin_panic_begin"] or time.monotonic() > deadline:
                    break
                time.sleep(0.05)
            checks += 1
            if names != ["main", "burdin_panic_begin"]:
                add_problem(f"the merged function breakpoints are `{names}`, not the client's plus the panic hook")

            # The program starts; the first step stops in drop glue, which the adapter steps out of silently, and the
            # second stop is in the user's code, which the client sees.
            client.send("configurationDone")
            stop = client.wait_for(lambda m: m.get("event") == "stopped")
            checks += 1
            if stop.get("body", {}).get("reason") != "step":
                add_problem(f"the forwarded stop is not the step's: {stop}")
            commands = [m.get("command") for m in backend_log(log)]
            checks += 1
            if commands.count("stackTrace") != 2:
                add_problem(f"the adapter did not check the top frame of both steps: {commands}")
            checks += 1
            if not any(command == "stepOut" for command in commands):
                add_problem("the adapter did not step out of generated code")

            client.send("disconnect")
            client.wait_for(lambda m: m.get("type") == "response" and m.get("command") == "disconnect")
            client.proc.stdin.close()
            checks += 1
            if proc.wait(timeout=60) != 0:
                add_problem(f"`burdin dap` exited {proc.returncode}: {harness.text(proc.stderr.read())}")
            checks += 1
            extra = []
            while True:
                message = client.read(timeout=5)
                if message is None:
                    break
                extra.append(message)
            if any(m.get("event") == "stopped" for m in extra):
                add_problem(f"a held stop reached the client: {extra}")
        finally:
            if proc.poll() is None:
                proc.kill()

        # DBG10 through the adapter: a test's index becomes the program's first argument.
        client, proc = start_session(work, wrapper, log)
        try:
            client.send("initialize", {})
            client.wait_for(lambda m: m.get("type") == "response" and m.get("command") == "initialize")
            client.send("launch", {"target": str(target), "test": "adds"})
            client.wait_for(lambda m: m.get("type") == "response" and m.get("command") == "launch")
            checks += 1
            if last_launch(log).get("args") != ["1"]:
                add_problem(f"`test: adds` did not become the dispatcher's index: {last_launch(log).get('args')}")
            client.send("disconnect")
            client.wait_for(lambda m: m.get("type") == "response" and m.get("command") == "disconnect")
            client.proc.stdin.close()
            proc.wait(timeout=60)
        finally:
            if proc.poll() is None:
                proc.kill()

checked = f"Checked the debug adapter ({checks} checks)"
if skips:
    for reason in skips:
        print(f"skip: {reason}")
if harness.report_problems(problems):
    sys.exit(1)
print("No problems.")
print(checked)
