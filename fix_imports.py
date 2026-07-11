import os

handler_dir = r'd:\tdcare\livekit\rust-sdks\rtc-patched\rtc\src\peer_connection\handler'

fixes = {
    'srtp.rs': ('use log::debug;', 'use log::{debug, trace};'),
    'datachannel.rs': ('use log::{debug, warn};', 'use log::{debug, trace, warn};'),
    'dtls.rs': ('use log::{debug, warn};', 'use log::{debug, trace, warn};'),
    'sctp.rs': ('use log::{debug, warn};', 'use log::{debug, trace, warn};'),
}

for f, (old, new) in fixes.items():
    path = os.path.join(handler_dir, f)
    with open(path, 'r') as fh:
        c = fh.read()
    c = c.replace(old, new)
    with open(path, 'w') as fh:
        fh.write(c)
    print(f'{f} done')

print('all done')
