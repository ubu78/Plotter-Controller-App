#!/usr/bin/env python3
"""Mock GRBL 1.1 board on a pty pair: prints the slave path, answers '?' with a status
report and every completed line with 'ok'. Line discipline mimics a real board."""
import os, pty, select, sys

master, slave = pty.openpty()
print(os.ttyname(slave), flush=True)

buf = b""
ov = [100.0]  # emulated feed override, %
wcs = [54]    # active work coordinate system
alarm = [False]  # emulated alarm lock: status shows Alarm, lines get no reply until $X
try:
    while True:
        r, _, _ = select.select([master], [], [], 0.2)
        if not r:
            continue
        try:
            d = os.read(master, 4096)
        except OSError:
            break
        for ch in d:
            if ch == 0x3F:  # '?' realtime status
                st = b"Alarm" if alarm[0] else b"Idle"
                os.write(master, b"<%s|MPos:10.000,5.000,0.000|WCO:0.000,0.000,0.000|Ov:%.0f,100,100>\r\n" % (st, ov[0]))
            elif ch == 0x18:  # soft reset
                ov[0] = 100.0
                os.write(master, b"Grbl 1.1h ['$' for help]\r\n")
            elif ch == 0x91:  # feed override +10%
                ov[0] = min(200.0, ov[0] + 10.0)
            elif ch == 0x92:  # feed override -10%
                ov[0] = max(10.0, ov[0] - 10.0)
            elif ch == 0x90:  # feed override 100%
                ov[0] = 100.0
            elif ch in (0x85, 0x21, 0x7E):  # jog cancel, hold, resume
                pass
            elif ch >= 0x80:  # any other realtime byte: consumed silently, never enters the line buffer
                pass
            elif ch == 0x0A:  # line completed
                line, buf = buf.strip(), b""
                if line == b"$ALARM":  # test hook: raise an alarm like a real board (no ok for the command itself)
                    alarm[0] = True
                    os.write(master, b"ALARM:1\r\n")
                elif line == b"$X":  # unlock always works, even from the alarm lock
                    alarm[0] = False
                    os.write(master, b"[Caution: Unlocked]\r\nok\r\n")
                elif alarm[0]:
                    pass  # locked board: the in-flight line is never answered, exactly like a real GRBL alarm
                elif line == b"$$":
                    os.write(master, b"$110=3000\r\n$111=3000\r\n$120=100\r\n$32=0\r\nok\r\n")
                elif line == b"$G":  # modal state report
                    os.write(master, b"[GC:G0 G%d G17 G21 G90 G94 M5 M9 T0 S0]\r\nok\r\n" % wcs[0])
                elif line in (b"G54", b"G55", b"G56", b"G57", b"G58", b"G59"):
                    wcs[0] = int(line[1:])
                    os.write(master, b"ok\r\n")
                elif line:
                    os.write(master, b"ok\r\n")
            else:
                buf += bytes([ch])
except KeyboardInterrupt:
    pass
