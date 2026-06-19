import paramiko, sys, json

ssh = paramiko.SSHClient()
ssh.set_missing_host_key_policy(paramiko.AutoAddPolicy())
ssh.connect('livekit.tdcare.cn', username='ubuntu', password='Tdcarefor123', timeout=10, look_for_keys=False, allow_agent=False)

# Check ohos events
# Get full OHOS participant active events
cmd = "echo Tdcarefor123 | sudo -S docker logs --tail 5000 livekit-livekit-1 2>&1 | grep 'participant active' | grep 'ohos_7467227352167095826' | tail -3"
stdin, stdout, stderr = ssh.exec_command(cmd, timeout=15)
print("=== ohos participant active ===")
print(stdout.read().decode('utf-8', errors='replace')[:5000])

# Get full mediaTrack published for Session 2 (PA_kX9kCEcpcr6V)
cmd_pub = "echo Tdcarefor123 | sudo -S docker logs --tail 10000 livekit-livekit-1 2>&1 | grep 'mediaTrack published' | grep 'PA_kX9kCEcpcr6V'"
stdin2, stdout2, stderr2 = ssh.exec_command(cmd_pub, timeout=15)
print("\n=== SESSION2 mediaTrack published ===")
out2 = stdout2.read().decode('utf-8', errors='replace')
for line in out2.strip().split('\n'):
    # Extract JSON payload
    idx = line.find('{')
    if idx < 0:
        continue
    try:
        j = json.loads(line[idx:])
        ti = j.get('trackInfo', {})
        layers = ti.get('layers', [])
        ssrcs_list = [l.get('ssrc') for l in layers if l.get('ssrc')]
        print(f"  kind={j.get('kind','?')} track={j.get('trackID','?')} ssrc={j.get('ssrc','?')} ssrcs(SDP)={j.get('ssrcs','?')} layers_ssrc={ssrcs_list} mime={j.get('mime','?')} fromSdp={j.get('fromSdp','?')}")
    except Exception as e:
        print(f"  {line[:300]}")
        print(f"  parse err: {e}")

# Check DTLS/SRTP errors or handshake events
cmd_dtls = "echo Tdcarefor123 | sudo -S docker logs --since 1h livekit-livekit-1 2>&1 | grep -i -E 'dtls|srtp|handshake' | grep -v 'ice' | tail -20"
stdin_dtls, stdout_dtls, stderr_dtls = ssh.exec_command(cmd_dtls, timeout=15)
print("\n=== DTLS/SRTP events (1h) ===")
print(stdout_dtls.read().decode('utf-8', errors='replace')[:5000])

# Check track publications in last 30 minutes
cmd2 = "echo Tdcarefor123 | sudo -S docker logs --since 30m livekit-livekit-1 2>&1 | grep 'mediaTrack published' | tail -10"
stdin2, stdout2, stderr2 = ssh.exec_command(cmd2, timeout=15)
print("\n=== Track publications (30m) ===")
out2 = stdout2.read().decode('utf-8', errors='replace')
for line in out2.strip().split('\n')[-10:]:
    if 'trackID' in line:
        try:
            j = json.loads(line[line.index('{'):])
            print(f"  p={j.get('participant','?')} kind={j.get('kind','?')} track={j.get('trackID','?')} mime={j.get('mime','?')}")
        except:
            print(f"  {line[:200]}")

# Check sender reports for any OHOS audio
cmd3 = "echo Tdcarefor123 | sudo -S docker logs --tail 20000 livekit-livekit-1 2>&1 | grep 'sender report' | grep -E 'TR_VCmz3sXuXCCcw7|TR_VCz25dUnCz7HBm|TR_AMigUtpioexFJx|TR_AMWPKQ6bpGCf5M|TR_VCpVxp8p2rUfxh' | tail -15"
stdin3, stdout3, stderr3 = ssh.exec_command(cmd3, timeout=15)
print("\n=== Audio sender reports ===")
out3 = stdout3.read().decode('utf-8', errors='replace')
for line in out3.strip().split('\n')[-5:]:
    if 'trackID' in line:
        try:
            j = json.loads(line[line.index('{'):])
            print(f"  track={j.get('trackID','?')} pkts={j.get('rtpStats',{}).get('packetsSeenPrimary','?')}")
        except:
            print(f"  {line[:200]}")

# Check video sender reports
cmd4 = "echo Tdcarefor123 | sudo -S docker logs --tail 20000 livekit-livekit-1 2>&1 | grep 'sender report' | grep -E 'TR_VCmz3sXuXCCcw7|TR_VCz25dUnCz7HBm|TR_VCDYaXR3LRK4Ku' | tail -15"
stdin4, stdout4, stderr4 = ssh.exec_command(cmd4, timeout=15)
print("\n=== Video sender reports ===")
out4 = stdout4.read().decode('utf-8', errors='replace')
for line in out4.strip().split('\n')[-5:]:
    if 'trackID' in line:
        try:
            j = json.loads(line[line.index('{'):])
            print(f"  track={j.get('trackID','?')} pkts={j.get('rtpStats',{}).get('packetsSeenPrimary','?')}")
        except:
            print(f"  {line[:200]}")

ssh.close()
