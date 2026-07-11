import os
import re

# Directories to process
dirs = [
    r'd:\tdcare\livekit\rust-sdks\rtc-patched\rtc\src\peer_connection\handler',
    r'd:\tdcare\livekit\rust-sdks\rtc-patched\rtc-dtls\src',
    r'd:\tdcare\livekit\rust-sdks\rtc-patched\rtc-sctp\src',
]

for dir_path in dirs:
    if not os.path.exists(dir_path):
        continue
    for root, _, files in os.walk(dir_path):
        for f in files:
            if not f.endswith('.rs'):
                continue
            path = os.path.join(root, f)
            with open(path, 'r') as fh:
                c = fh.read()
            orig = c
            # Change debug! to trace! for verbose logs
            c = re.sub(r'\bdebug!\("recv ', 'trace!("recv ', c)
            c = re.sub(r'\bdebug!\("send ', 'trace!("send ', c)
            c = re.sub(r'\bdebug!\("Send ', 'trace!("Send ', c)
            c = re.sub(r'\bdebug!\("Recv ', 'trace!("Recv ', c)
            c = re.sub(r'\bdebug!\("\[Server\]', 'trace!("[Server]', c)
            c = re.sub(r'\bdebug!\("\[handshake', 'trace!("[handshake', c)
            c = re.sub(r'\bdebug!\("Flight ', 'trace!("Flight ', c)
            c = re.sub(r'\bdebug!\("association_handle', 'trace!("association_handle', c)
            c = re.sub(r'\bdebug!\("ice selected', 'trace!("ice selected', c)
            c = re.sub(r'\bdebug!\("recv dtls', 'trace!("recv dtls', c)
            c = re.sub(r'\bdebug!\("send dtls', 'trace!("send dtls', c)
            c = re.sub(r'\bdebug!\("recv sctp', 'trace!("recv sctp', c)
            c = re.sub(r'\bdebug!\("recv SCTP', 'trace!("recv SCTP', c)
            c = re.sub(r'\bdebug!\("Received ', 'trace!("Received ', c)
            if c != orig:
                # Ensure trace is imported
                if 'use log::' in c:
                    c = re.sub(r'use log::\{([^}]*)\}', lambda m: 'use log::{' + (m.group(1) if 'trace' in m.group(1) else m.group(1) + ', trace') + '}', c)
                    c = re.sub(r'use log::(\w+);', r'use log::{\1, trace};', c)
                    # Deduplicate
                    c = re.sub(r'use log::\{([^,]+), trace, trace\}', r'use log::{\1, trace}', c)
                    c = re.sub(r'use log::\{trace, trace\}', r'use log::{trace}', c)
                with open(path, 'w') as fh:
                    fh.write(c)
                print(f'Fixed: {path}')

print('done')
