import paramiko, sys, json

try:
    ssh = paramiko.SSHClient()
    ssh.set_missing_host_key_policy(paramiko.AutoAddPolicy())
    ssh.connect('livekit.tdcare.cn', username='ubuntu', password='Tdcarefor123', timeout=10, look_for_keys=False, allow_agent=False)

    # Check recent ohos events with larger window
    cmd = "echo Tdcarefor123 | sudo -S docker logs --tail 5000 livekit-livekit-1 2>&1 | grep -E 'ohos_746' | tail -20"
    s, o, e = ssh.exec_command(cmd, timeout=15)
    print("=== Recent ohos events ===", flush=True)
    out = o.read().decode('utf-8', errors='replace')
    for line in out.strip().split('\n')[-15:]:
        print(line[:250], flush=True)

    # Check for any participant joined/left in last hour
    cmd2 = "echo Tdcarefor123 | sudo -S docker logs --since 1h livekit-livekit-1 2>&1 | grep -iE 'participant.*joined|participant.*left|participant.*disconnect|RTC session' | tail -15"
    s2, o2, e2 = ssh.exec_command(cmd2, timeout=15)
    print("\n=== Participant join/leave (1h) ===", flush=True)
    out2 = o2.read().decode('utf-8', errors='replace')
    for line in out2.strip().split('\n')[-10:]:
        print(line[:300], flush=True)

    # Check for audio/video track publications in last hour
    cmd3 = "echo Tdcarefor123 | sudo -S docker logs --since 1h livekit-livekit-1 2>&1 | grep -E 'mediaTrack published|trackPublished' | tail -10"
    s3, o3, e3 = ssh.exec_command(cmd3, timeout=15)
    print("\n=== Track publications (1h) ===", flush=True)
    out3 = o3.read().decode('utf-8', errors='replace')
    for line in out3.strip().split('\n')[-10:]:
        # Extract key fields
        if 'trackID' in line:
            try:
                j = json.loads(line[line.index('{'):])
                print(f"  participant={j.get('participant','?')} kind={j.get('kind','?')} track={j.get('trackID','?')} mime={j.get('mime','?')}", flush=True)
            except:
                print(f"  {line[:200]}", flush=True)

    # Check sender reports for any audio
    cmd4 = "echo Tdcarefor123 | sudo -S docker logs --tail 3000 livekit-livekit-1 2>&1 | grep 'sender report' | grep 'audio' | tail -5"
    s4, o4, e4 = ssh.exec_command(cmd4, timeout=15)
    print("\n=== Latest audio sender reports ===", flush=True)
    out4 = o4.read().decode('utf-8', errors='replace')
    for line in out4.strip().split('\n'):
        if 'trackID' in line:
            try:
                j = json.loads(line[line.index('{'):])
                print(f"  track={j.get('trackID','?')} pkts={j.get('rtpStats',{}).get('packetsSeenPrimary','?')} bitrate={j.get('rtpStats',{}).get('bitrate','?')}", flush=True)
            except:
                print(f"  {line[:200]}", flush=True)

    # Check active rooms
    cmd5 = "echo Tdcarefor123 | sudo -S docker logs --tail 500 livekit-livekit-1 2>&1 | grep -E 'room.*rrt_dept|RM_' | grep -vE 'SDP|signal|candidate|offer|answer' | tail -5"
    s5, o5, e5 = ssh.exec_command(cmd5, timeout=10)
    print("\n=== Room activity ===", flush=True)
    out5 = o5.read().decode('utf-8', errors='replace')
    print(out5[:2000] if out5 else "(none)", flush=True)

    ssh.close()
except Exception as ex:
    print(f"ERROR: {ex}", flush=True)
    import traceback
    traceback.print_exc()
    sys.exit(1)
