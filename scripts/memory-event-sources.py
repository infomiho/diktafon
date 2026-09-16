#!/usr/bin/env python3
"""Measure macOS keyboard event allocation without sending keystrokes."""
import argparse
import ctypes
import os
import subprocess

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--repeat', type=int, default=1000)
parser.add_argument('--reuse', action='store_true')
args = parser.parse_args()
if args.repeat < 1:
    parser.error('--repeat must be positive')
cg = ctypes.CDLL('/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics')
cf = ctypes.CDLL('/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation')
cg.CGEventSourceCreate.argtypes = [ctypes.c_int32]
cg.CGEventSourceCreate.restype = ctypes.c_void_p
cg.CGEventCreateKeyboardEvent.argtypes = [ctypes.c_void_p, ctypes.c_uint16, ctypes.c_bool]
cg.CGEventCreateKeyboardEvent.restype = ctypes.c_void_p
cf.CFRelease.argtypes = [ctypes.c_void_p]
cf.CFRelease.restype = None


def source():
    result = cg.CGEventSourceCreate(-1)
    if not result:
        raise RuntimeError('CGEventSourceCreate failed')
    return result


def snapshot(cycle):
    result = subprocess.run(['footprint', '-p', str(os.getpid())],
                            capture_output=True, text=True, check=True, timeout=30)
    print(cycle, next(line for line in result.stdout.splitlines() if 'Footprint:' in line),
          flush=True)


shared = source() if args.reuse else None
try:
    snapshot(0)
    for cycle in range(1, args.repeat + 1):
        current = shared or source()
        try:
            event = cg.CGEventCreateKeyboardEvent(current, 9, True)
            if not event:
                raise RuntimeError('CGEventCreateKeyboardEvent failed')
            cf.CFRelease(event)
        finally:
            if not shared:
                cf.CFRelease(current)
        if cycle in (100, args.repeat):
            snapshot(cycle)
finally:
    if shared:
        cf.CFRelease(shared)
