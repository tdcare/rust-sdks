import os

handler_dir = r'd:\tdcare\livekit\rust-sdks\rtc-patched\rtc\src\peer_connection\handler'
files = ['ice.rs', 'srtp.rs', 'dtls.rs', 'sctp.rs', 'datachannel.rs']

replacements = [
    ('debug!("Bypass ice write', 'trace!("Bypass ice write'),
    ('debug!("bypass ice read', 'trace!("bypass ice read'),
    ('debug!("srtp write', 'trace!("srtp write'),
    ('debug!("srtp read', 'trace!("srtp read'),
    ('debug!("Bypass srtp write', 'trace!("Bypass srtp write'),
    ('debug!("bypass srtp read', 'trace!("bypass srtp read'),
    ('debug!("Bypass sctp write', 'trace!("Bypass sctp write'),
    ('debug!("bypass sctp read', 'trace!("bypass sctp read'),
    ('debug!("Bypass dtls write', 'trace!("Bypass dtls write'),
    ('debug!("bypass dtls read', 'trace!("bypass dtls read'),
    ('debug!("bypass DataChannel read', 'trace!("bypass DataChannel read'),
    ('debug!("bypass DataChannel write', 'trace!("bypass DataChannel write'),
]

for f in files:
    path = os.path.join(handler_dir, f)
    with open(path, 'r') as fh:
        c = fh.read()
    for old, new in replacements:
        c = c.replace(old, new)
    with open(path, 'w') as fh:
        fh.write(c)
    print(f'{f} done')

print('all done')
