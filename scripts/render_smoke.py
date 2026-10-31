#!/usr/bin/env python3
import argparse
import base64
import ctypes
import json
import os
from pathlib import Path
import re
import shutil
import socket
import struct
import subprocess
import threading
import time
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from urllib.request import urlopen
from urllib.parse import urlparse

ROOT = Path(__file__).resolve().parents[1]
LOGS = ROOT / "runtime" / "logs"
MARKER = "VELDMAP_SMOKE "
BAD_LOG = re.compile(
    r"panicked at|unhandled promise|validation error|gpu validation|runtime error|"
    r"initialization failed|sqlite.*error|codec.*error|error:.*(?:wgpu|vulkan|webgpu)|\bERROR:",
    re.IGNORECASE,
)
CHROME_ENVIRONMENT_ERROR = re.compile(
    r"ERROR:dbus/object_proxy\.cc.*ServiceUnknown.*org\.freedesktop\.UPower|"
    r"ERROR:google_apis/gcm/engine/(?:registration_request|mcs_client)\.cc"
)


class SmokeFailure(RuntimeError):
    pass


class Scenario:
    def __init__(self, controller):
        self.controller = controller
        self.measurements = {}
        self.started = time.monotonic()

    def wait(self, predicate, description, timeout=20):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            status = self.controller.latest()
            if status is not None and predicate(status):
                return status
            self.controller.check_alive()
            self.controller.pump(0.05)
        status = self.controller.latest()
        raise SmokeFailure(f"timed out waiting for {description}: {status}; logs: {self.controller.lines[-20:]}")

    def perform(self, name, action, predicate, description):
        started = time.monotonic()
        action()
        self.controller.refresh()
        status = self.wait(predicate, description)
        self.measurements[f"{name}_ms"] = round((time.monotonic() - started) * 1000, 1)
        return status

    def click(self, control):
        point = self.controller.latest()["controls"][control]
        self.controller.click(point)

    def idle(self, name, seconds=1.0):
        before = self.controller.latest()
        sequence = before["sequence"]
        draws = [before[name]["draws"] for name in ("viewer", "globe")]
        self.controller.idle(seconds)
        after = self.controller.latest()
        if after["sequence"] != sequence:
            raise SmokeFailure(f"redraw during {name}: sequences {sequence} -> {after['sequence']}")
        if [after[name]["draws"] for name in ("viewer", "globe")] != draws:
            raise SmokeFailure(f"draw counters changed during {name}")
        self.measurements[f"{name}_ms"] = round(seconds * 1000, 1)

    def settle_after_startup(self):
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            sequence = self.controller.latest()["sequence"]
            self.controller.idle(0.5)
            if self.controller.latest()["sequence"] == sequence:
                return
        raise SmokeFailure("application did not become idle after startup")

    def run(self):
        initial = self.wait(lambda status: status["ready"], "readiness")
        if initial["tile_uploads"] != 1 or initial["tile_upload_bytes"] != 256:
            raise SmokeFailure("expected exactly one 256-byte tile upload")
        if initial["retained_cpu_tile_bytes"] != 0 or initial["cache_entries"] != 1:
            raise SmokeFailure("unexpected retained CPU pixels or cache entries")
        handles = {initial[name]["tile_handle"] for name in ("viewer", "globe")}
        if len(handles) != 1 or 0 in handles:
            raise SmokeFailure("viewer and globe did not share one tile allocation")
        if any(initial[name]["clip_modes"] & 1 == 0 for name in ("viewer", "globe")):
            raise SmokeFailure("initial clipped rendering was not observed")
        self.controller.require_backend(initial)
        self.settle_after_startup()
        self.idle("initial_idle")

        self.perform(
            "widget_response",
            lambda: self.click("interact"),
            lambda status: status["interactions"] >= 1,
            "widget input",
        )
        for index, name in enumerate(("viewer", "globe")):
            counter = f"{name}_drags"
            self.perform(
                f"{name}_drag_response",
                lambda name=name: self.controller.drag(self.controller.latest()[name]["input"]),
                lambda status, counter=counter: status[counter] >= 1,
                f"{name} pointer drag",
            )
            if self.controller.latest()["tile_uploads"] != 1:
                raise SmokeFailure("pointer input triggered another tile upload")

        self.perform(
            "clip_response",
            lambda: self.click("clip"),
            lambda status: not status["clip_enabled"],
            "clip toggle",
        )
        self.controller.refresh()
        self.wait(
            lambda status: all(status[name]["clip_modes"] == 3 for name in ("viewer", "globe")),
            "clipped and unclipped draw modes",
        )
        previous = self.controller.latest()
        self.perform(
            "resize_response",
            lambda: self.controller.resize(900, 650),
            lambda status: all(
                status[name]["screen_px"] != previous[name]["screen_px"]
                and status[name]["size_changes"] > previous[name]["size_changes"]
                for name in ("viewer", "globe")
            ),
            "surface reconfiguration",
        )
        previous_scale = self.controller.latest()["pixels_per_point"]
        self.perform(
            "scale_response",
            lambda: self.click("scale"),
            lambda status: status["scale_changes"] >= 1
            and status["pixels_per_point"] > previous_scale,
            "scale-factor change",
        )
        self.controller.refresh()
        self.wait(
            lambda status: status["viewer"]["pixels_per_point"] > previous_scale
            and status["globe"]["pixels_per_point"] > previous_scale,
            "scaled GPU callbacks",
        )
        if self.controller.name == "browser":
            started = time.monotonic()
            screenshot = self.controller.screenshot()
            captured = {"capture": self.controller.capture_stats}
            self.measurements["capture_response_ms"] = round((time.monotonic() - started) * 1000, 1)
        else:
            self.perform(
                "capture_response",
                lambda: self.click("capture"),
                lambda status: status["capture"]["completed"] >= 1,
                "diagnostic capture",
            )
            captured = self.controller.latest()
            if captured["capture"]["error"] is not None:
                raise SmokeFailure(captured["capture"]["error"])
            screenshot = self.controller.screenshot()
        for name in ("viewer", "globe"):
            view = captured["capture"][name]
            if view["samples"] < 1000 or view["colors"] < 4:
                raise SmokeFailure(f"capture did not contain visible {name} output: {view}")
            if any(view[color] < 20 for color in ("red", "green", "blue", "yellow")):
                raise SmokeFailure(f"capture missed fixture colors in {name}: {view}")
        self.settle_after_startup()
        self.idle("final_idle")
        final = self.controller.latest()
        return {
            "passed": True,
            "target": self.controller.name,
            "adapter": final["adapter"],
            "backend": final["backend"],
            "software_gpu": getattr(self.controller, "software_gpu", final["software_gpu"]),
            "scenario_ms": round((time.monotonic() - self.started) * 1000, 1),
            "max_action_ms": max(value for name, value in self.measurements.items() if not name.endswith("_idle_ms")),
            "measurements": self.measurements,
            "screenshot": str(screenshot.relative_to(ROOT)),
            "tile_uploads": final["tile_uploads"],
            "tile_upload_bytes": final["tile_upload_bytes"],
            "retained_cpu_tile_bytes": final["retained_cpu_tile_bytes"],
            "cache_entries": final["cache_entries"],
            "uniform_uploads": final["uniform_uploads"],
            "uniform_upload_bytes": final["uniform_upload_bytes"],
            "surface_reconfigurations": final["surface_reconfigurations"],
            "capture": captured["capture"],
            "environment_notes": getattr(self.controller, "environment_notes", []),
            "final": final,
        }


class X11:
    def __init__(self):
        self.x11 = ctypes.CDLL("libX11.so.6")
        self.xtst = ctypes.CDLL("libXtst.so.6")
        self.configure()
        self.display = self.x11.XOpenDisplay(None)
        if not self.display:
            raise SmokeFailure("cannot open the X11 display")
        self.root = self.x11.XDefaultRootWindow(self.display)

    def configure(self):
        display = ctypes.c_void_p
        window = ctypes.c_ulong
        self.x11.XOpenDisplay.argtypes = [ctypes.c_char_p]
        self.x11.XOpenDisplay.restype = display
        self.x11.XDefaultRootWindow.argtypes = [display]
        self.x11.XDefaultRootWindow.restype = window
        self.x11.XQueryTree.argtypes = [display, window, ctypes.POINTER(window), ctypes.POINTER(window), ctypes.POINTER(ctypes.POINTER(window)), ctypes.POINTER(ctypes.c_uint)]
        self.x11.XFetchName.argtypes = [display, window, ctypes.POINTER(ctypes.c_char_p)]
        self.x11.XFree.argtypes = [ctypes.c_void_p]
        self.x11.XRaiseWindow.argtypes = [display, window]
        self.x11.XSetInputFocus.argtypes = [display, window, ctypes.c_int, ctypes.c_ulong]
        self.x11.XResizeWindow.argtypes = [display, window, ctypes.c_uint, ctypes.c_uint]
        self.x11.XTranslateCoordinates.argtypes = [display, window, window, ctypes.c_int, ctypes.c_int, ctypes.POINTER(ctypes.c_int), ctypes.POINTER(ctypes.c_int), ctypes.POINTER(window)]
        self.x11.XFlush.argtypes = [display]
        self.x11.XSync.argtypes = [display, ctypes.c_int]
        self.x11.XStringToKeysym.argtypes = [ctypes.c_char_p]
        self.x11.XStringToKeysym.restype = ctypes.c_ulong
        self.x11.XKeysymToKeycode.argtypes = [display, ctypes.c_ulong]
        self.x11.XKeysymToKeycode.restype = ctypes.c_uint
        self.x11.XCloseDisplay.argtypes = [display]
        self.xtst.XTestFakeMotionEvent.argtypes = [display, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_ulong]
        self.xtst.XTestFakeButtonEvent.argtypes = [display, ctypes.c_uint, ctypes.c_int, ctypes.c_ulong]
        self.xtst.XTestFakeKeyEvent.argtypes = [display, ctypes.c_uint, ctypes.c_int, ctypes.c_ulong]

    def find_window(self, title):
        matches = []
        def visit(window):
            name = ctypes.c_char_p()
            if self.x11.XFetchName(self.display, window, ctypes.byref(name)) and name.value:
                if name.value.decode(errors="replace") == title:
                    matches.append(window)
                self.x11.XFree(name)
            root, parent = ctypes.c_ulong(), ctypes.c_ulong()
            children = ctypes.POINTER(ctypes.c_ulong)()
            count = ctypes.c_uint()
            if not self.x11.XQueryTree(self.display, window, ctypes.byref(root), ctypes.byref(parent), ctypes.byref(children), ctypes.byref(count)):
                return
            for index in range(count.value):
                visit(children[index])
            if children:
                self.x11.XFree(children)
        visit(self.root)
        if not matches:
            raise SmokeFailure(f"no X11 window named {title}")
        return matches[-1]

    def origin(self, window):
        x, y, child = ctypes.c_int(), ctypes.c_int(), ctypes.c_ulong()
        self.x11.XTranslateCoordinates(self.display, window, self.root, 0, 0, ctypes.byref(x), ctypes.byref(y), ctypes.byref(child))
        return x.value, y.value

    def move(self, x, y):
        self.xtst.XTestFakeMotionEvent(self.display, -1, round(x), round(y), 0)
        self.x11.XSync(self.display, 0)

    def button(self, pressed):
        self.xtst.XTestFakeButtonEvent(self.display, 1, int(pressed), 0)
        self.x11.XSync(self.display, 0)

    def close_window(self, window):
        class Data(ctypes.Union):
            _fields_ = [("bytes", ctypes.c_char * 20), ("shorts", ctypes.c_short * 10), ("longs", ctypes.c_long * 5)]
        class ClientMessage(ctypes.Structure):
            _fields_ = [("type", ctypes.c_int), ("serial", ctypes.c_ulong), ("send_event", ctypes.c_int), ("display", ctypes.c_void_p), ("window", ctypes.c_ulong), ("message_type", ctypes.c_ulong), ("format", ctypes.c_int), ("data", Data)]
        class Event(ctypes.Union):
            _fields_ = [("client", ClientMessage), ("padding", ctypes.c_long * 24)]
        self.x11.XInternAtom.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_int]
        self.x11.XInternAtom.restype = ctypes.c_ulong
        self.x11.XSendEvent.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_long, ctypes.c_void_p]
        event = Event()
        event.client.type = 33
        event.client.display = self.display
        event.client.window = window
        event.client.message_type = self.x11.XInternAtom(self.display, b"WM_PROTOCOLS", 0)
        event.client.format = 32
        event.client.data.longs[0] = self.x11.XInternAtom(self.display, b"WM_DELETE_WINDOW", 0)
        self.x11.XSendEvent(self.display, window, 0, 0, ctypes.byref(event))
        self.x11.XSync(self.display, 0)

    def close(self):
        self.x11.XCloseDisplay(self.display)


class NativeController:
    name = "native"

    def __init__(self):
        self.log_path = LOGS / "native-smoke.log"
        self.log_file = self.log_path.open("w", encoding="utf-8")
        self.lines = []
        self.latest_status = None
        self.lock = threading.Lock()
        env = os.environ.copy()
        env.update({"VELDMAP_SMOKE": "1", "WINIT_X11_SCALE_FACTOR": "1.25"})
        env.pop("WAYLAND_DISPLAY", None)
        self.process = subprocess.Popen(
            [str(ROOT / "target" / "debug" / "veldmap-native")],
            cwd=ROOT,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            bufsize=1,
        )
        self.reader = threading.Thread(target=self.read_logs, daemon=True)
        self.reader.start()
        self.x11 = X11()
        self.x11.configure()
        self.window = None
        self.closed = False

    def read_logs(self):
        for line in self.process.stdout:
            line = line.rstrip()
            self.lines.append(line)
            self.log_file.write(line + "\n")
            self.log_file.flush()
            if line.startswith(MARKER):
                with self.lock:
                    self.latest_status = json.loads(line[len(MARKER):])
        self.log_file.close()

    def latest(self):
        with self.lock:
            return self.latest_status.copy() if self.latest_status else None

    def check_alive(self):
        if self.process.poll() is not None:
            raise SmokeFailure(f"native process exited with {self.process.returncode}: {self.lines[-20:]}")

    def pump(self, seconds):
        time.sleep(seconds)

    def require_backend(self, status):
        if status["backend"] != "Vulkan":
            raise SmokeFailure(f"expected Vulkan, got {status['backend']}")
        if self.window is None:
            self.window = self.x11.find_window("VeldMap")
            self.x11.x11.XRaiseWindow(self.x11.display, self.window)
            self.x11.x11.XSetInputFocus(self.x11.display, self.window, 2, 0)
            self.x11.x11.XSync(self.x11.display, 0)

    def physical(self, point):
        status = self.latest()
        origin = self.x11.origin(self.window)
        scale = status["pixels_per_point"]
        return origin[0] + point[0] * scale, origin[1] + point[1] * scale

    def click(self, point):
        self.x11.move(*self.physical(point))
        time.sleep(0.05)
        self.x11.button(True)
        time.sleep(0.08)
        self.x11.button(False)

    def drag(self, point):
        x, y = self.physical(point)
        self.x11.move(x, y)
        time.sleep(0.05)
        self.x11.button(True)
        for step in range(1, 7):
            self.x11.move(x + step * 5, y + step * 2)
            time.sleep(0.03)
        self.x11.button(False)

    def refresh(self):
        self.x11.move(*self.physical([4, 4]))

    def resize(self, width, height):
        scale = self.latest()["native_pixels_per_point"]
        self.x11.x11.XResizeWindow(self.x11.display, self.window, round(width * scale), round(height * scale))
        self.x11.x11.XSync(self.x11.display, 0)

    def idle(self, seconds):
        time.sleep(seconds)

    def screenshot(self):
        path = LOGS / "native-smoke.ppm"
        deadline = time.monotonic() + 10
        while not path.exists() and time.monotonic() < deadline:
            time.sleep(0.05)
        if not path.exists() or path.stat().st_size < 1000:
            raise SmokeFailure("native diagnostic screenshot was not written")
        return path

    def close(self):
        if self.closed:
            return
        self.x11.close_window(self.window)
        self.process.wait(timeout=15)
        self.reader.join(timeout=5)
        self.x11.close()
        self.closed = True
        if self.process.returncode != 0 or "VELDMAP_SHUTDOWN" not in self.lines:
            raise SmokeFailure("native shutdown was not clean")
        check_logs(self.lines)

    def cleanup(self):
        if self.closed:
            return
        self.process.terminate()
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait()
        self.reader.join(timeout=2)
        self.x11.close()
        self.closed = True


class WebSocket:
    def __init__(self, url):
        parsed = urlparse(url)
        self.socket = socket.create_connection((parsed.hostname, parsed.port), timeout=10)
        key = base64.b64encode(os.urandom(16)).decode()
        request = (
            f"GET {parsed.path} HTTP/1.1\r\nHost: {parsed.hostname}:{parsed.port}\r\n"
            f"Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\n"
            f"Sec-WebSocket-Version: 13\r\nOrigin: http://{parsed.hostname}:{parsed.port}\r\n\r\n"
        )
        self.socket.sendall(request.encode())
        response = b""
        while b"\r\n\r\n" not in response:
            response += self.socket.recv(4096)
        if b" 101 " not in response.split(b"\r\n", 1)[0]:
            raise SmokeFailure(f"WebSocket upgrade failed: {response[:200]}")

    def send(self, text, opcode=1):
        payload = text.encode() if isinstance(text, str) else text
        mask = os.urandom(4)
        length = len(payload)
        header = bytes([0x80 | opcode])
        if length < 126:
            header += bytes([0x80 | length])
        elif length < 65536:
            header += bytes([0x80 | 126]) + struct.pack("!H", length)
        else:
            header += bytes([0x80 | 127]) + struct.pack("!Q", length)
        masked = bytes(value ^ mask[index % 4] for index, value in enumerate(payload))
        self.socket.sendall(header + mask + masked)

    def receive(self):
        first, second = self.read_exact(2)
        opcode = first & 0x0F
        length = second & 0x7F
        if length == 126:
            length = struct.unpack("!H", self.read_exact(2))[0]
        elif length == 127:
            length = struct.unpack("!Q", self.read_exact(8))[0]
        mask = self.read_exact(4) if second & 0x80 else None
        payload = self.read_exact(length)
        if mask:
            payload = bytes(value ^ mask[index % 4] for index, value in enumerate(payload))
        if opcode == 9:
            self.send(payload, opcode=10)
            return self.receive()
        if opcode == 8:
            raise SmokeFailure("browser WebSocket closed")
        return payload.decode() if opcode == 1 else None

    def read_exact(self, length):
        data = b""
        while len(data) < length:
            chunk = self.socket.recv(length - len(data))
            if not chunk:
                raise SmokeFailure("browser WebSocket disconnected")
            data += chunk
        return data

    def close(self):
        self.socket.close()


class Cdp:
    def __init__(self, url, on_event):
        self.socket = WebSocket(url)
        self.next_id = 1
        self.on_event = on_event

    def call(self, method, params=None):
        request_id = self.next_id
        self.next_id += 1
        self.socket.send(json.dumps({"id": request_id, "method": method, "params": params or {}}))
        while True:
            message = json.loads(self.socket.receive())
            if message.get("id") == request_id:
                if "error" in message:
                    raise SmokeFailure(f"CDP {method}: {message['error']}")
                return message.get("result", {})
            self.event(message)

    def pump(self, seconds):
        self.socket.socket.settimeout(seconds)
        try:
            message = self.socket.receive()
            if message:
                self.event(json.loads(message))
        except socket.timeout:
            pass
        finally:
            self.socket.socket.settimeout(10)

    def event(self, message):
        if "method" in message:
            self.on_event(message)


class BrowserController:
    name = "browser"

    def __init__(self):
        self.latest_status = None
        self.lines = []
        self.console_issues = []
        self.shutdown_seen = False
        self.closed = False
        chrome = find_chrome()
        windows_chrome = os.name == "nt" or chrome.lower().endswith(".exe")
        if windows_chrome and os.name != "nt":
            raise SmokeFailure("Windows Chrome verification requires a Windows-native checkout")
        self.debug_host = "127.0.0.1"
        self.server = self.start_server()
        self.port = self.server.server_address[1]
        self.process_log = (LOGS / "browser-process.log").open("w", encoding="utf-8")
        self.gpu_mode = os.environ.get("VELDMAP_BROWSER_GPU", "native")
        if self.gpu_mode not in ("native", "swiftshader"):
            raise SmokeFailure("VELDMAP_BROWSER_GPU must be native or swiftshader")
        self.software_gpu = self.gpu_mode == "swiftshader"
        profile = ROOT / "target" / "tmp" / "chrome-render-smoke"
        shutil.rmtree(profile, ignore_errors=True)
        profile.mkdir(parents=True, exist_ok=True)
        command = [
            chrome,
            "--remote-debugging-port=0",
            "--remote-allow-origins=*",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-dev-shm-usage",
            "--disable-background-networking",
            "--disable-features=BatteryStatus,PushMessaging",
            "--disable-push-messaging",
            "--disable-notifications",
        ]
        if not windows_chrome or self.software_gpu:
            command.append("--enable-unsafe-webgpu")
        if self.gpu_mode == "native":
            if not windows_chrome:
                command += [
                    "--enable-features=Vulkan,VulkanFromANGLE,DefaultANGLEVulkan",
                    "--use-angle=vulkan",
                    "--use-vulkan=native",
                    "--ignore-gpu-blocklist",
                    "--ozone-platform=x11",
                ]
        else:
            warning = "VELDMAP_GPU_WARNING software WebGPU was explicitly selected; no hardware GPU is in use"
            print(warning, flush=True)
            self.lines.append(warning)
            command += [
                "--enable-features=Vulkan",
                "--use-angle=vulkan",
                "--use-vulkan=swiftshader",
                "--use-webgpu-adapter=swiftshader",
                "--disable-vulkan-surface",
                "--enable-unsafe-swiftshader",
            ]
            if not windows_chrome:
                command.append("--ozone-platform=x11")
        command += ["--window-size=1000,720", f"--user-data-dir={profile}", "about:blank"]
        env = os.environ.copy()
        if self.software_gpu:
            swiftshader_icd = Path(chrome).parent / "vk_swiftshader_icd.json"
            if not swiftshader_icd.exists():
                raise SmokeFailure(f"missing bundled SwiftShader ICD: {swiftshader_icd}")
            env["VK_ICD_FILENAMES"] = str(swiftshader_icd)
        local_libs = ROOT / "target" / "tools" / "browser-deps" / "root" / "usr" / "lib" / "x86_64-linux-gnu"
        if local_libs.exists():
            env["LD_LIBRARY_PATH"] = str(local_libs) + os.pathsep + env.get("LD_LIBRARY_PATH", "")
        self.process = subprocess.Popen(command, cwd=ROOT, env=env, stdout=self.process_log, stderr=subprocess.STDOUT)
        debug_port = self.debug_port(profile)
        self.debug_port = debug_port
        targets = self.json(debug_port, "list")
        page = next(target for target in targets if target["type"] == "page")
        self.cdp = Cdp(page["webSocketDebuggerUrl"], self.event)
        for domain in ("Page.enable", "Runtime.enable", "Log.enable"):
            self.cdp.call(domain)
        self.cdp.call("Emulation.setDeviceMetricsOverride", {"width": 1000, "height": 720, "deviceScaleFactor": 1.25, "mobile": False})
        page_host = "localhost" if windows_chrome else "127.0.0.1"
        self.cdp.call("Page.navigate", {"url": f"http://{page_host}:{self.port}/index.html?smoke=1"})

    def start_server(self):
        directory = ROOT / "target" / "web"
        class Handler(SimpleHTTPRequestHandler):
            def __init__(self, *args, **kwargs):
                super().__init__(*args, directory=str(directory), **kwargs)
            def end_headers(self):
                self.send_header("Cross-Origin-Opener-Policy", "same-origin")
                self.send_header("Cross-Origin-Embedder-Policy", "require-corp")
                super().end_headers()
            def log_message(self, *args):
                pass
        server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        return server

    def debug_port(self, profile):
        path = profile / "DevToolsActivePort"
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            self.check_alive()
            if path.exists():
                return int(path.read_text().splitlines()[0])
            time.sleep(0.05)
        raise SmokeFailure("Chrome debugging port was not created")

    def json(self, port, path):
        with urlopen(f"http://{self.debug_host}:{port}/json/{path}", timeout=5) as response:
            return json.load(response)

    def event(self, message):
        method = message["method"]
        params = message.get("params", {})
        if method == "Runtime.consoleAPICalled":
            values = [arg.get("value", arg.get("description", "")) for arg in params.get("args", [])]
            line = " ".join(str(value) for value in values)
            self.lines.append(line)
            if params.get("type") == "error":
                self.console_issues.append(line)
            if line.startswith(MARKER):
                self.latest_status = json.loads(line[len(MARKER):])
            if line == "VELDMAP_SHUTDOWN":
                self.shutdown_seen = True
        elif method == "Runtime.exceptionThrown":
            self.console_issues.append(json.dumps(params))
            self.lines.append(json.dumps(params))
        elif method == "Log.entryAdded":
            entry = params.get("entry", {})
            self.lines.append(entry.get("text", ""))
            if entry.get("level") == "error":
                self.console_issues.append(entry.get("text", ""))

    def latest(self):
        return self.latest_status.copy() if self.latest_status else None

    def check_alive(self):
        if self.process.poll() is not None:
            raise SmokeFailure(f"Chrome exited with {self.process.returncode}")
        if self.console_issues:
            raise SmokeFailure(f"browser errors: {self.console_issues}")

    def pump(self, seconds):
        self.cdp.pump(seconds)

    def require_backend(self, status):
        if status["backend"] != "BrowserWebGpu":
            raise SmokeFailure(f"expected WebGPU, got {status['backend']}")

    def event_point(self, point):
        zoom = self.latest()["zoom_factor"]
        return point[0] * zoom, point[1] * zoom

    def click(self, point):
        point = self.event_point(point)
        self.cdp.call("Input.dispatchMouseEvent", {"type": "mouseMoved", "x": point[0], "y": point[1]})
        self.cdp.call("Input.dispatchMouseEvent", {"type": "mousePressed", "x": point[0], "y": point[1], "button": "left", "clickCount": 1})
        time.sleep(0.08)
        self.cdp.call("Input.dispatchMouseEvent", {"type": "mouseReleased", "x": point[0], "y": point[1], "button": "left", "clickCount": 1})

    def drag(self, point):
        x, y = self.event_point(point)
        self.cdp.call("Input.dispatchMouseEvent", {"type": "mouseMoved", "x": x, "y": y})
        self.cdp.call("Input.dispatchMouseEvent", {"type": "mousePressed", "x": x, "y": y, "button": "left", "clickCount": 1})
        for step in range(1, 7):
            self.cdp.call("Input.dispatchMouseEvent", {"type": "mouseMoved", "x": x + step * 5, "y": y + step * 2, "button": "left"})
            time.sleep(0.03)
        self.cdp.call("Input.dispatchMouseEvent", {"type": "mouseReleased", "x": x + 30, "y": y + 12, "button": "left", "clickCount": 1})

    def refresh(self):
        x, y = self.event_point([4, 4])
        self.cdp.call("Input.dispatchMouseEvent", {"type": "mouseMoved", "x": x, "y": y})

    def resize(self, width, height):
        self.cdp.call("Emulation.setDeviceMetricsOverride", {"width": width, "height": height, "deviceScaleFactor": 1.25, "mobile": False})

    def idle(self, seconds):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            self.pump(min(0.05, deadline - time.monotonic()))

    def screenshot(self):
        result = self.cdp.call("Page.captureScreenshot", {"format": "png"})
        data = base64.b64decode(result["data"])
        path = LOGS / "browser-smoke.png"
        path.write_bytes(data)
        width, height, pixels = decode_png(data)
        status = self.latest()
        self.capture_stats = {
            name: capture_view(pixels, width, height, status[name]["rect"], status["pixels_per_point"])
            for name in ("viewer", "globe")
        }
        return path

    def close(self):
        if self.closed:
            return
        self.cdp.call("Runtime.evaluate", {"expression": "window.veldmapShutdown()"})
        deadline = time.monotonic() + 5
        while not self.shutdown_seen and time.monotonic() < deadline:
            self.cdp.pump(0.05)
        version = self.json(self.debug_port, "version")
        browser = Cdp(version["webSocketDebuggerUrl"], self.event)
        try:
            browser.call("Browser.close")
        except SmokeFailure:
            pass
        self.process.wait(timeout=15)
        browser.socket.close()
        self.cdp.socket.close()
        self.server.shutdown()
        self.server.server_close()
        self.process_log.close()
        self.closed = True
        if not self.shutdown_seen:
            raise SmokeFailure("browser application shutdown was not observed")
        process_lines = (LOGS / "browser-process.log").read_text(encoding="utf-8", errors="replace").splitlines()
        self.environment_notes = check_browser_logs(self.lines + self.console_issues, process_lines)

    def cleanup(self):
        if self.closed:
            return
        self.process.terminate()
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait()
        self.server.shutdown()
        self.server.server_close()
        self.process_log.close()
        self.closed = True


def decode_png(data):
    import zlib

    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise SmokeFailure("browser screenshot is not a PNG")
    offset = 8
    compressed = bytearray()
    header = None
    while offset < len(data):
        length = struct.unpack("!I", data[offset:offset + 4])[0]
        kind = data[offset + 4:offset + 8]
        payload = data[offset + 8:offset + 8 + length]
        offset += length + 12
        if kind == b"IHDR":
            header = struct.unpack("!IIBBBBB", payload)
        elif kind == b"IDAT":
            compressed.extend(payload)
        elif kind == b"IEND":
            break
    if header is None:
        raise SmokeFailure("browser screenshot has no header")
    width, height, depth, color_type, _, _, interlace = header
    channels = {0: 1, 2: 3, 6: 4}.get(color_type)
    if depth != 8 or channels is None or interlace != 0:
        raise SmokeFailure(f"unsupported screenshot format: {header}")
    raw = zlib.decompress(compressed)
    stride = width * channels
    decoded = bytearray(height * stride)
    previous = bytearray(stride)
    source = 0
    for row in range(height):
        filter_type = raw[source]
        source += 1
        current = bytearray(raw[source:source + stride])
        source += stride
        for index in range(stride):
            left = current[index - channels] if index >= channels else 0
            above = previous[index]
            upper_left = previous[index - channels] if index >= channels else 0
            if filter_type == 1:
                current[index] = (current[index] + left) & 255
            elif filter_type == 2:
                current[index] = (current[index] + above) & 255
            elif filter_type == 3:
                current[index] = (current[index] + ((left + above) // 2)) & 255
            elif filter_type == 4:
                estimate = left + above - upper_left
                distances = (abs(estimate - left), abs(estimate - above), abs(estimate - upper_left))
                predictor = (left, above, upper_left)[distances.index(min(distances))]
                current[index] = (current[index] + predictor) & 255
            elif filter_type != 0:
                raise SmokeFailure(f"unsupported PNG filter {filter_type}")
        decoded[row * stride:(row + 1) * stride] = current
        previous = current
    return width, height, (decoded, channels)


def capture_view(pixel_data, width, height, rect, scale):
    pixels, channels = pixel_data
    left = max(0, round(rect[0] * scale) + 8)
    top = max(0, round(rect[1] * scale) + 8)
    right = min(width, round(rect[2] * scale) - 8)
    bottom = min(height, round(rect[3] * scale) - 8)
    result = {"samples": 0, "red": 0, "green": 0, "blue": 0, "yellow": 0}
    colors = set()
    for y in range(top, bottom):
        for x in range(left, right):
            offset = (y * width + x) * channels
            red, green, blue = pixels[offset:offset + 3]
            result["samples"] += 1
            colors.add((red, green, blue))
            result["red"] += int(red > 180 and green < 120 and blue < 120)
            result["green"] += int(red < 120 and green > 120 and blue < 150)
            result["blue"] += int(red < 120 and green < 180 and blue > 150)
            result["yellow"] += int(red > 180 and green > 140 and blue < 100)
    result["colors"] = len(colors)
    return result


def find_chrome():
    if os.environ.get("CHROME_BIN"):
        return os.environ["CHROME_BIN"]
    if os.name == "nt":
        for path in (Path("C:/Program Files/Google/Chrome/Application/chrome.exe"), Path("C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe")):
            if path.exists():
                return str(path)
    workspace = sorted((ROOT / "target" / "tools" / "playwright-browsers").glob("chromium-*/chrome-linux64/chrome"))
    if workspace:
        return str(workspace[-1])
    for name in ("google-chrome", "chromium", "chromium-browser"):
        path = shutil.which(name)
        if path:
            return path
    cached = sorted(Path.home().glob(".cache/ms-playwright/chromium-*/chrome-linux64/chrome"))
    if cached:
        return str(cached[-1])
    raise SmokeFailure("no Chrome executable found; set CHROME_BIN")


def check_logs(lines):
    issues = [line for line in lines if BAD_LOG.search(line)]
    if issues:
        raise SmokeFailure(f"unexpected log diagnostics: {issues}")


def check_browser_logs(application_lines, process_lines):
    notes = [line for line in process_lines if CHROME_ENVIRONMENT_ERROR.search(line)]
    unexpected = [
        line for line in process_lines
        if "ERROR:" in line and not CHROME_ENVIRONMENT_ERROR.search(line)
    ]
    issues = [
        line for line in application_lines + unexpected
        if BAD_LOG.search(line) and not CHROME_ENVIRONMENT_ERROR.search(line)
    ]
    if issues:
        raise SmokeFailure(f"unexpected log diagnostics: {issues}")
    return notes


def build(target):
    if target == "native":
        subprocess.run(["cargo", "build", "-p", "veldmap", "--locked"], cwd=ROOT, check=True)
        return
    subprocess.run(["cargo", "build", "-p", "veldmap", "--target", "wasm32-unknown-unknown", "--locked"], cwd=ROOT, check=True)
    web = ROOT / "target" / "web"
    web.mkdir(parents=True, exist_ok=True)
    shutil.copy2(ROOT / "apps" / "veldmap" / "web" / "index.html", web / "index.html")
    subprocess.run([
        "wasm-bindgen",
        "--target", "web",
        "--out-dir", str(web),
        "--out-name", "veldmap",
        str(ROOT / "target" / "wasm32-unknown-unknown" / "debug" / "veldmap.wasm"),
    ], cwd=ROOT, check=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("target", choices=("native", "browser"))
    parser.add_argument("--no-build", action="store_true")
    args = parser.parse_args()
    LOGS.mkdir(parents=True, exist_ok=True)
    if not args.no_build:
        build(args.target)
    controller = NativeController() if args.target == "native" else BrowserController()
    result = None
    try:
        result = Scenario(controller).run()
        controller.close()
        result["log_issues"] = []
    finally:
        controller.cleanup()
    path = LOGS / f"{args.target}-smoke-result.json"
    path.write_text(json.dumps(result, indent=2) + "\n")
    print(f"VELDMAP_SMOKE_PASS {path.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
