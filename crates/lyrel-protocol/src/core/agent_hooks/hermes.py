# lyrel agent-hook hermes v2
"""Hermes 0.22.2 plain-CLI observers, under Hermes' own plugin opt-in."""
import json
import os
import subprocess
import sys
import threading

_RUNNER = __LYREL_RUNNER__
_children = set()
_running_children = {}
_reported = None
_turn_id = None
_lock = threading.RLock()


def _diagnostic(message):
    print(f"lyrel: Hermes {message}", file=sys.stderr)


def _emit(event, payload):
    if not os.environ.get("LYREL"):
        return False
    try:
        result = subprocess.run(
            [_RUNNER, "agent-hook", "hermes", event],
            input=json.dumps(payload, ensure_ascii=False).encode("utf-8"),
            stdout=subprocess.DEVNULL, timeout=5,
            creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
        )
        if result.returncode:
            _diagnostic("observer delivery failed")
            return False
        return True
    except (OSError, subprocess.TimeoutExpired) as exc:
        _diagnostic(f"observer delivery failed: {type(exc).__name__}")
        return False


def _child(child_session_id=None, parent_session_id=None, parent_turn_id=None, **kwargs):
    if child_session_id:
        with _lock:
            _children.add(child_session_id)
            if parent_session_id == _reported:
                _running_children[child_session_id] = parent_turn_id
            elif parent_session_id in _running_children:
                _running_children[child_session_id] = _running_children[parent_session_id]


def _child_end(child_session_id=None, **kwargs):
    with _lock:
        _running_children.pop(child_session_id, None)
        # Async completion queues a NEW system turn. It is not that turn's
        # final result and cannot settle the parent pane on its own.


def _main(session_id, platform=None):
    # Tool observers have no platform. They may only address the already
    # observed CLI session, never promote a child or gateway identity.
    return bool(session_id and session_id not in _children and
                session_id == _reported and platform in (None, "cli"))


def _identity(session_id=None, platform=None, parent_session_id=None, event="session-start", **kwargs):
    global _reported
    if platform != "cli" or parent_session_id or not os.environ.get("LYREL"):
        return
    with _lock:
        if session_id in _children or (_reported is not None and session_id == _reported):
            return
        if not isinstance(session_id, str) or not session_id.strip():
            _diagnostic("observer reported no persisted session identity")
            return
        from hermes_constants import get_hermes_home
        from hermes_cli.profiles import get_active_profile_name
        payload = {
            "session_id": session_id,
            "cwd": os.getcwd(),
            "source": "hermes-cli-observer",
            "session_scope": {
                "profile": get_active_profile_name(),
                "environment": {"HERMES_HOME": str(get_hermes_home())},
            },
        }
        if _emit(event, payload):
            _reported = session_id


def _start(session_id=None, platform=None, turn_id=None, parent_session_id=None, **kwargs):
    global _turn_id
    if parent_session_id:
        _child(child_session_id=session_id)
        return
    with _lock:
        _identity(session_id=session_id, platform=platform)
        if _main(session_id, platform):
            _turn_id = turn_id
            _emit("prompt-submit", {"session_id": session_id})


def _end(session_id=None, platform=None, completed=None, failed=None,
         interrupted=None, turn_exit_reason=None, turn_id=None, **kwargs):
    # on_session_end is a finalized TURN, not on_session_finalize (identity
    # teardown), post_llm_call (also abnormal text), or per-attempt API errors.
    # In 0.22.2 this follows verification, persistence and compaction and carries
    # all three verdicts even for interrupted and failed turns.
    global _turn_id
    with _lock:
        if platform != "cli" or not turn_id or turn_id != _turn_id or session_id in _children:
            return
        # Compression can advance the stored identity inside this SAME turn.
        # A notification carries it without resetting outstanding input state.
        _identity(session_id=session_id, platform=platform, event="notification")
        if not _main(session_id, platform):
            return
        if turn_id in _running_children.values():
            _diagnostic("turn finalized with delegated work pending; awaiting the native continuation turn")
            return
        if not all(isinstance(value, bool) for value in (completed, failed, interrupted)):
            _diagnostic("turn end has no authoritative verdict; status retained")
            return
        outcome = "interrupted" if interrupted else "failed" if failed or not completed else "succeeded"
        result = {"outcome": outcome}
        if isinstance(turn_exit_reason, str) and turn_exit_reason:
            result["reason"] = turn_exit_reason[:240]
        if _emit("stop", {"session_id": session_id, "result": result}):
            _turn_id = None


def _input(resolved=False, session_id=None, session_key=None, platform=None,
           request_id=None, kind=None, **kwargs):
    # These two observers share a native UUID and fire only around an actual
    # human wait. Approval observers also fire for smart/automatic decisions
    # and must NOT open another request. No prompts, passwords or answers leave.
    with _lock:
        if platform != "cli" or session_id in _children:
            return
        if session_id and not _main(session_id, platform):
            return
        if not session_id and session_key not in (None, "", "default", _reported):
            _diagnostic("human input has an unbound routing key; status retained")
            return
        if not isinstance(request_id, str) or not request_id:
            _diagnostic("human input has no request identity; status retained")
            return
        request_kind = {"approval": "permission", "sudo": "permission", "clarify": "question"}.get(kind)
        if request_kind is None:
            _diagnostic("human input kind is unsupported; status retained")
            return
        payload = {"request": {"id": request_id, "kind": request_kind, "resolved": resolved}}
        if session_id:
            payload["session_id"] = session_id
        _emit("tool-complete" if resolved else "permission-request", payload)


def _input_resolved(**kwargs):
    _input(resolved=True, **kwargs)


def _command(surface=None, session_key=None, platform=None, **kwargs):
    if surface == "cli":
        _identity(session_id=session_key, platform=platform)


def register(ctx):
    ctx.register_hook("subagent_start", _child)
    ctx.register_hook("subagent_stop", _child_end)
    ctx.register_hook("on_session_start", _identity)
    ctx.register_hook("on_session_reset", _identity)
    ctx.register_hook("pre_llm_call", _start)
    ctx.register_hook("on_session_end", _end)
    ctx.register_hook("on_human_input_request", _input)
    ctx.register_hook("on_human_input_resolved", _input_resolved)
    ctx.register_hook("pre_command", _command)
