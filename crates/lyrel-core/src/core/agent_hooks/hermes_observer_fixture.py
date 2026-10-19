"""Controlled upstream observer/IPC boundary; not real Hermes runtime evidence."""
import json
import os
from pathlib import Path
import sys
import types
from unittest.mock import patch

home = types.ModuleType("hermes_constants")
home.get_hermes_home = lambda: Path("/isolated/hermes")
profiles = types.ModuleType("hermes_cli.profiles")
profiles.get_active_profile_name = lambda: "default"
sys.modules["hermes_constants"] = home
sys.modules["hermes_cli.profiles"] = profiles

namespace = {}
source = Path(__file__).with_name("hermes.py").read_text(encoding="utf-8")
exec(compile(source.replace("__LYREL_RUNNER__", repr("fixture-runner")), "hermes.py", "exec"), namespace)
hooks = {}
namespace["register"](types.SimpleNamespace(register_hook=lambda name, callback: hooks.__setitem__(name, callback)))
events = []


def deliver(argv, *, input, **kwargs):
    payload = json.loads(input)
    events.append({"v": 1, "agent": "hermes", "event": argv[-1], **payload})
    return types.SimpleNamespace(returncode=0)


with patch.dict(os.environ, {"LYREL": "1"}), patch("subprocess.run", side_effect=deliver):
    hooks["pre_llm_call"](session_id="main", platform="cli", turn_id="turn-1", user_message="PRIVATE-PROMPT")
    hooks["on_human_input_request"](session_id="", session_key="default", platform="cli", request_id="a", kind="approval", prompt="PRIVATE-COMMAND")
    hooks["on_human_input_request"](session_id="main", platform="cli", request_id="b", kind="clarify", prompt="PRIVATE-QUESTION")
    hooks["on_human_input_resolved"](session_id="", session_key="default", platform="cli", request_id="a", kind="approval", outcome="denied")
    hooks["subagent_start"](child_session_id="child")
    hooks["pre_llm_call"](session_id="child", parent_session_id="main", platform="cli", turn_id="child-turn")
    hooks["on_session_end"](session_id="child", platform="cli", turn_id="child-turn", completed=True, failed=False, interrupted=False)
    hooks["on_human_input_resolved"](session_id="main", platform="cli", request_id="b", kind="clarify", outcome="cancelled", answer="PRIVATE-ANSWER")
    # Compression promotes the stored ID within a correlated parent turn.
    hooks["on_session_end"](session_id="compressed-main", platform="cli", turn_id="turn-1", completed=True, failed=False, interrupted=False, turn_exit_reason="text_response")
    hooks["pre_llm_call"](session_id="compressed-main", platform="cli", turn_id="turn-2")
    # Late terminal callbacks cannot finish the subsequent turn.
    hooks["on_session_end"](session_id="main", platform="cli", turn_id="turn-1", completed=True, failed=False, interrupted=False)
    hooks["on_session_end"](session_id="compressed-main", platform="cli", turn_id="turn-2", completed=False, failed=False, interrupted=True, turn_exit_reason="user_interrupt")
    hooks["pre_llm_call"](session_id="compressed-main", platform="cli", turn_id="turn-3")
    hooks["on_session_end"](session_id="compressed-main", platform="cli", turn_id="turn-3", completed=False, failed=True, interrupted=False, turn_exit_reason="max_iterations")

    hooks["pre_llm_call"](session_id="compressed-main", platform="cli", turn_id="turn-4")
    hooks["subagent_start"](parent_session_id="compressed-main", parent_turn_id="turn-4", child_session_id="async-child")
    hooks["on_session_end"](session_id="compressed-main", platform="cli", turn_id="turn-4", completed=True, failed=False, interrupted=False)
    hooks["subagent_stop"](child_session_id="async-child", child_status="completed")
    hooks["pre_llm_call"](session_id="compressed-main", platform="cli", turn_id="turn-5")
    hooks["on_session_end"](session_id="compressed-main", platform="cli", turn_id="turn-5", completed=True, failed=False, interrupted=False)
encoded = json.dumps(events, ensure_ascii=False)
assert "PRIVATE-" not in encoded
assert "child" not in encoded
sys.stdout.buffer.write((encoded + "\n").encode("utf-8"))
