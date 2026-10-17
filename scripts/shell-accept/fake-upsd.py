#!/usr/bin/env python3
"""A stand-in for NUT's upsd, for shell-accept.sh only.

Answers LIST UPS and LIST VAR the way upsd does, for one UPS named
"testups", on 127.0.0.1 at the given port. The UPS's status is read
from a file at each question, so the script can put it on battery.

    fake-upsd.py <port> <status file>
"""

import socketserver
import sys

PORT = int(sys.argv[1])
STATUS = sys.argv[2]


def variables():
    with open(STATUS, encoding="utf-8") as f:
        status = f.read().strip()
    return {
        "device.model": 'Test "UPS" 1500',
        "ups.status": status,
        "ups.realpower": "245",
        "ups.realpower.nominal": "900",
        "ups.load": "27",
        "battery.charge": "100",
        "battery.runtime": "2280",
        "input.voltage": "121.0",
    }


def quote(value):
    return '"' + value.replace("\\", "\\\\").replace('"', '\\"') + '"'


class Handler(socketserver.StreamRequestHandler):
    def handle(self):
        for raw in self.rfile:
            question = raw.decode().strip()
            if question == "LIST UPS":
                lines = ["BEGIN LIST UPS", 'UPS testups "A test UPS"', "END LIST UPS"]
            elif question == "LIST VAR testups":
                lines = ["BEGIN LIST VAR testups"]
                lines += [f"VAR testups {k} {quote(v)}" for k, v in variables().items()]
                lines.append("END LIST VAR testups")
            elif question == "LOGOUT":
                return
            else:
                lines = ["ERR UNKNOWN-COMMAND"]
            self.wfile.write(("\n".join(lines) + "\n").encode())


class Server(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True


Server(("127.0.0.1", PORT), Handler).serve_forever()
