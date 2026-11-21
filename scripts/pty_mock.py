#!/usr/bin/env python3
"""Mock GRBL 1.1 board on a pty pair: prints the slave path, answers '?' with a status
report and every completed line with 'ok'. Line discipline mimics a real board.
0.5.34: $J= jogs enter a transient Jog state with Y rolling negative; 0x85 cancels the
jog where it stands; G10 L20 P<d> X0 Y0 (Z0) shifts WCO so the current position reads zero."""
import os, pty, select, sys, time

master, slave = pty.openpty()
print(os.ttyname(slave), flush=True)

buf = b""
ov = [100.0]  # emulated feed override, %
wcs = [54]    # active work coordinate system
alarm = [False]  # emulated alarm lock: status shows Alarm, lines get no reply until $X
mpos = [10.0, 5.0, 0.0]  # machine position: the status reports MPos, WPos = MPos - WCO
wco = [0.0, 0.0, 0.0]
jog = [None]  # deadline of the $J in flight (None = not jogging)
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
                if alarm[0]:
                    st = b"Alarm"
                    jog[0] = None
                elif jog[0] is not None and time.time() < jog[0]:
                    mpos[1] -= 1.5  # the jog rolls Y toward zero
                    st = b"Jog"
                else:
                    jog[0] = None
                    st = b"Idle"
                os.write(master, b"<%s|MPos:%.3f,%.3f,%.3f|WCO:%.3f,%.3f,%.3f|Ov:%.0f,100,100>\r\n" % (st, mpos[0], mpos[1], mpos[2], wco[0], wco[1], wco[2], ov[0]))
            elif ch == 0x18:  # soft reset
                ov[0] = 100.0
                jog[0] = None
                mpos = [10.0, 5.0, 0.0]
                wco = [0.0, 0.0, 0.0]
                os.write(master, b"Grbl 1.1h ['$' for help]\r\n")
            elif ch == 0x91:  # feed override +10%
                ov[0] = min(200.0, ov[0] + 10.0)
            elif ch == 0x92:  # feed override -10%
                ov[0] = max(10.0, ov[0] - 10.0)
            elif ch == 0x90:  # feed override 100%
                ov[0] = 100.0
            elif ch == 0x85:  # jog cancel: the board stands where it stands
                jog[0] = None
            elif ch in (0x21, 0x7E):  # hold, resume
                pass
            elif ch >= 0x80:  # any other realtime byte: consumed silently, never enters the line buffer
                pass
            elif ch == 0x0A:  # line completed
                line, buf = buf.strip(), b""
                if line == b"$ALARM":  # test hook: raise an alarm like a real board (no ok for the command itself)
                    alarm[0] = True
                    jog[0] = None
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
                elif line.startswith(b"$J="):  # 0.5.34: jog — a short Jog window with Y rolling to -Y
                    jog[0] = time.time() + 1.0
                    os.write(master, b"ok\r\n")
                elif line.startswith(b"G10 L20"):  # zero button: the current position reads 0 in WCS
                    wco = list(mpos)
                    os.write(master, b"ok\r\n")
                elif line:
                    os.write(master, b"ok\r\n")
            else:
                buf += bytes([ch])
except KeyboardInterrupt:
    pass
