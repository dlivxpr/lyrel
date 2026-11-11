"""Image-only bridge to pinned Hermes' real public JSON-RPC dispatcher.

No monkeypatch, mock handler, PNG encoder, terminal writer or model request.
The caller provides an isolated HERMES_HOME and native dependency interpreter.
"""
import json
import os
from pathlib import Path
import sys

source = Path(sys.argv[sys.argv.index("--source") + 1]).resolve(strict=True)
sys.path.insert(0, str(source))
from agent.terminal_image import local_image_path, tool_image_path

request = json.load(sys.stdin)
assert request["method"] == "image.render"
params = request["params"]
image = str(Path(params["path"]).resolve(strict=True))
home = Path(os.environ["HERMES_HOME"]).resolve(strict=True)
assert Path(image).parent == home.parent, "Only the fixture's owned images are eligible"
assert local_image_path(image) == image
# The native tool gate, not text/path guessing: successful named local fields
# only. Exercise attachment and eligible tool input with the SAME native RPC.
assert tool_image_path("vision_analyze", {"image_url": image}, {}, is_error=False) == image
assert tool_image_path("image_generate", {}, {"image": image}, is_error=False) == image
assert tool_image_path("browser_exec", {}, {"screenshot_path": image}, is_error=False) == image
assert tool_image_path("read_file", {"path": image}, {"text": image}, is_error=False) is None
assert tool_image_path("vision_analyze", {"image_url": image}, {}, is_error=True) is None
assert tool_image_path("vision_analyze", {"image_url": "https://example.invalid/x.png"}, {}, is_error=False) is None
assert local_image_path(str(home.parent / "literal.svg")) is None
if Path(image).name == "image-1.png":
    params["path"] = tool_image_path("image_generate", {}, {"image": image}, is_error=False)
from tui_gateway import server
response = server.handle_request(request)
assert response is not None and "error" not in response, response
# Importing server redirects sys.stdout to stderr; only its transport owns RPC stdout.
assert server.write_json(response), "Native gateway failed to write its image.render response"
