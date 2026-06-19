import os
import re

# Process all .rs files in rtc-patched
base = r'd:\tdcare\livekit\rust-sdks\rtc-patched'
count = 0
for root, _, files in os.walk(base):
    for f in files:
        if not f.endswith('.rs'):
            continue
        if 'test' in f:
            continue
        path = os.path.join(root, f)
        with open(path, 'r') as fh:
            c = fh.read()
        orig = c
        # Change all debug! to trace! EXCEPT for specific important messages
        lines = c.split('\n')
        new_lines = []
        for line in lines:
            if 'debug!' in line and 'peer_addr' not in line:
                # Skip important debug messages we want to keep
                keep = any(k in line for k in [
                    'signaling state',
                    'connection state',
                    'negotiation',
                    'add_track',
                    'do_add_track',
                    'publisher offer',
                    'publisher answer',
                    'subscriber offer',
                    'offer sent',
                    'answer received',
                    'local track',
                    'remote track',
                    'Connected',
                ])
                if not keep:
                    line = line.replace('debug!', 'trace!')
            new_lines.append(line)
        c = '\n'.join(new_lines)
        if c != orig:
            # Ensure trace is imported
            if 'use log::{' in c and 'trace' not in c.split('use log::{')[1].split('}')[0]:
                c = c.replace('use log::{', 'use log::{trace, ', 1)
            elif 'use log::debug;' in c:
                c = c.replace('use log::debug;', 'use log::{debug, trace};')
            elif 'use log::warn;' in c and 'trace' not in c:
                c = c.replace('use log::warn;', 'use log::{warn, trace};')
            with open(path, 'w') as fh:
                fh.write(c)
            count += 1
            relpath = os.path.relpath(path, base)
            print(f'Fixed: {relpath}')

print(f'Total: {count} files')
