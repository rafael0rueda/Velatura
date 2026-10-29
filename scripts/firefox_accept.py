#!/usr/bin/env python3
"""Firefox must compute the colours of Velatura's stylesheet for its window.

  scripts/firefox_accept.py <profile> themed <velatura.css>
  scripts/firefox_accept.py <profile> stock <velatura.css>

Starts Firefox headless on the profile with Marionette on a port of its own
choosing, and asks its window, in the browser's own context, for what it
computed. Themed: every variable in the stylesheet has its value on the
root, and the window body, the tab strip and the toolbar are drawn in the
theme's colours. Stock: the toolbar is not drawn in the theme's colour.
"""
import json
import os
import re
import socket
import subprocess
import sys
import time

profile, mode, sheet = sys.argv[1:4]
wanted = dict(re.findall(r"^\s*(--[a-z-]+): (#[0-9a-f]{6}) !important;$", open(sheet).read(), re.M))
if len(wanted) < 10:
    sys.exit(f"{sheet} has only {len(wanted)} variables")


def rgb(hex_color):
    r, g, b = (int(hex_color[i:i + 2], 16) for i in (1, 3, 5))
    return f"rgb({r}, {g}, {b})"


port_file = os.path.join(profile, "MarionetteActivePort")
if os.path.exists(port_file):
    os.remove(port_file)
# The browser's own context needs system access: asked for with the flag up
# to Firefox 158, and from 159 with the variable as well.
firefox = subprocess.Popen(
    ["firefox", "--headless", "--no-remote", "--profile", profile, "--marionette", "--remote-allow-system-access"],
    env={**os.environ, "MOZ_REMOTE_ALLOW_SYSTEM_ACCESS": "1"},
    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
try:
    for _ in range(300):
        if os.path.exists(port_file) and open(port_file).read().strip():
            break
        time.sleep(0.1)
    else:
        sys.exit("Firefox did not start Marionette")
    port = int(open(port_file).read())
    conn = socket.create_connection(("127.0.0.1", port), timeout=60)
    stream = conn.makefile("rb")

    def receive():
        size = b""
        while (c := stream.read(1)) != b":":
            if not c:
                sys.exit("Marionette closed the connection")
            size += c
        return json.loads(stream.read(int(size)))

    serial = 0

    def command(name, params):
        global serial
        serial += 1
        data = json.dumps([0, serial, name, params]).encode()
        conn.sendall(str(len(data)).encode() + b":" + data)
        _, _, error, result = receive()
        if error:
            sys.exit(f"{name}: {error}")
        return result

    receive()
    command("WebDriver:NewSession", {"capabilities": {}})
    command("Marionette:SetContext", {"value": "chrome"})
    script = """
        const [names] = arguments;
        const root = getComputedStyle(document.documentElement);
        const body = getComputedStyle(document.body);
        const bar = getComputedStyle(document.getElementById("nav-bar"));
        return {
            version: Services.appinfo.version,
            vars: Object.fromEntries(names.map(n => [n, root.getPropertyValue(n).trim()])),
            body: [body.backgroundColor, body.color],
            toolbar: [bar.backgroundColor, bar.color],
        };
    """
    # The window's colours may still be on their way in a transition.
    for _ in range(30):
        seen = command("WebDriver:ExecuteScript", {"script": script, "args": [list(wanted)]})["value"]
        if mode == "stock" or seen["toolbar"][0] == rgb(wanted["--toolbar-background-color"]):
            break
        time.sleep(0.2)
    command("Marionette:Quit", {"flags": ["eForceQuit"]})
finally:
    try:
        firefox.wait(timeout=30)
    except subprocess.TimeoutExpired:
        firefox.kill()

problems = []
if mode == "themed":
    for name, value in wanted.items():
        if seen["vars"][name].lower() != value:
            problems.append(f"{name} is {seen['vars'][name]!r}, not {value}")
    expect = {
        "window body": (seen["body"], ["--toolbox-background-color", "--toolbox-text-color"]),
        "toolbar": (seen["toolbar"], ["--toolbar-background-color", "--toolbar-text-color"]),
    }
    for part, (got, names) in expect.items():
        if got != [rgb(wanted[n]) for n in names]:
            problems.append(f"the {part} is drawn {got}, not in {[wanted[n] for n in names]}")
elif seen["toolbar"][0] == rgb(wanted["--toolbar-background-color"]):
    problems.append("the toolbar still has the theme's colour")
if problems:
    print(f"Firefox {seen['version']}: " + "; ".join(problems))
    sys.exit(1)
print(f"Firefox {seen['version']} draws its window in "
      + ("the theme's colours" if mode == "themed" else "its own colours again"))
