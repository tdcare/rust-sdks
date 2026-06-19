import paramiko, json

ssh = paramiko.SSHClient()
ssh.set_missing_host_key_policy(paramiko.AutoAddPolicy())
ssh.connect('livekit.tdcare.cn', username='ubuntu', password='Tdcarefor123', timeout=10, look_for_keys=False, allow_agent=False)

# Search for sender reports for the OHOS tracks specifically
# Use larger tail and grep for the track IDs
cmd = "echo Tdcarefor123 | sudo -S docker logs --tail 20000 livekit-livekit-1 2>&1 | grep 'sender report' | grep -E 'TR_VCmz3sXuXCCcw7|TR_VCz25dUnCz7HBm|TR_AMigUtpioexFJx' | tail -10"
stdin, stdout, stderr = ssh.exec_command(cmd, timeout=20)
print('=== OHOS sender reports ===')
out = stdout.read().decode('utf-8', errors='replace')
if out.strip():
    for line in out.strip().split('\n'):
        try:
            j = json.loads(line[line.index('{'):])
            print(f"  track={j.get('trackID','?')} pkts={j.get('rtpStats',{}).get('packetsSeenPrimary','?')}")
        except:
            print(f"  {line[:200]}")
else:
    print('  (NO sender reports found for OHOS tracks!)')

# Also search for any RTP-related errors
cmd2 = "echo Tdcarefor123 | sudo -S docker logs --tail 20000 livekit-livekit-1 2>&1 | grep 'ohos_7467227352167095826' | grep -iE 'error|warn|fail|invalid' | grep -v 'dtls timeout\|data channel' | tail -10"
stdin2, stdout2, stderr2 = ssh.exec_command(cmd2, timeout=20)
print('\n=== OHOS errors (excluding dc/dtls) ===')
out2 = stdout2.read().decode('utf-8', errors='replace')
print(out2[:2000] if out2.strip() else '(none)')

ssh.close()
