import paramiko
import sys

try:
    ssh = paramiko.SSHClient()
    ssh.set_missing_host_key_policy(paramiko.AutoAddPolicy())
    ssh.connect('livekit.tdcare.cn', username='ubuntu', password='Tdcarefor123', timeout=10, look_for_keys=False, allow_agent=False)
    print("SSH connected OK", flush=True)

    # Check for the NEW ohos session (PID 19222 started ~11:14)
    cmd = "echo Tdcarefor123 | sudo -S docker logs --tail 2000 livekit-livekit-1 2>&1 | grep -E 'ohos_7467227352167095.*audio|ohos_7467227352167095.*track.*published|mediaTrack published.*ohos.*audio' | tail -10"
    s, o, e = ssh.exec_command(cmd, timeout=15)
    print("=== Latest OHOS audio track publications ===", flush=True)
    out = o.read().decode('utf-8', errors='replace')
    print(out[:3000] if out else "(none)", flush=True)

    # Check sender reports for any ohos audio track
    cmd2 = "echo Tdcarefor123 | sudo -S docker logs --tail 2000 livekit-livekit-1 2>&1 | grep 'sender report' | grep -E 'ohos.*audio|audio.*ohos|TR_AM.*ohos' | tail -5"
    s2, o2, e2 = ssh.exec_command(cmd2, timeout=15)
    print("\n=== Sender reports for OHOS audio ===", flush=True)
    out2 = o2.read().decode('utf-8', errors='replace')
    print(out2[:3000] if out2 else "(none)", flush=True)

    # Check all sender reports (audio) recently
    cmd3 = "echo Tdcarefor123 | sudo -S docker logs --tail 2000 livekit-livekit-1 2>&1 | grep 'sender report' | grep 'audio' | tail -5"
    s3, o3, e3 = ssh.exec_command(cmd3, timeout=15)
    print("\n=== All audio sender reports ===", flush=True)
    out3 = o3.read().decode('utf-8', errors='replace')
    for line in out3.strip().split('\n')[-5:]:
        # Print just key stats from each report
        if 'trackID' in line:
            import json
            try:
                # Extract JSON part
                json_start = line.index('{')
                data = json.loads(line[json_start:])
                track = data.get('trackID', '?')
                kind = data.get('kind', '?')
                rtp = data.get('rtpStats', {})
                pkts = rtp.get('packetsSeenPrimary', '?')
                lost = rtp.get('packetsLost', '?')
                bitrate = rtp.get('bitrate', '?')
                print(f"  track={track} kind={kind} pkts={pkts} lost={lost} bitrate={bitrate}", flush=True)
            except:
                print(f"  (parse error: {line[:200]})", flush=True)
        else:
            print(f"  {line[:200]}", flush=True)

    ssh.close()
except Exception as ex:
    print(f"ERROR: {ex}", flush=True)
    import traceback
    traceback.print_exc()
    sys.exit(1)
