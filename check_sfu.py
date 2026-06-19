import paramiko
import sys, json

try:
    ssh = paramiko.SSHClient()
    ssh.set_missing_host_key_policy(paramiko.AutoAddPolicy())
    ssh.connect('livekit.tdcare.cn', username='ubuntu', password='Tdcarefor123', timeout=10, look_for_keys=False, allow_agent=False)
    print("SSH connected", flush=True)

    # Get the room info via API
    cmd = 'echo Tdcarefor123 | sudo -S docker exec livekit-livekit-1 curl -s http://localhost:7880/ 2>&1'
    s, o, e = ssh.exec_command(cmd, timeout=10)
    print("=== Root ===", flush=True)
    print(o.read().decode('utf-8', errors='replace')[:500], flush=True)

    # List rooms
    cmd2 = 'echo Tdcarefor123 | sudo -S docker exec livekit-livekit-1 curl -s http://localhost:7880/v1/room/list 2>&1'
    s2, o2, e2 = ssh.exec_command(cmd2, timeout=10)
    print("\n=== Room List ===", flush=True)
    out2 = o2.read().decode('utf-8', errors='replace')
    print(out2[:1000], flush=True)

    # Get participants for rrt_dept_5
    cmd3 = 'echo Tdcarefor123 | sudo -S docker exec livekit-livekit-1 curl -s "http://localhost:7880/v1/room/participants?room=rrt_dept_5" 2>&1'
    s3, o3, e3 = ssh.exec_command(cmd3, timeout=10)
    print("\n=== Participants in rrt_dept_5 ===", flush=True)
    out3 = o3.read().decode('utf-8', errors='replace')
    if out3:
        try:
            data = json.loads(out3)
            parts = data.get('participants', [])
            for p in parts:
                identity = p.get('identity', '?')
                sid = p.get('sid', '?')
                tracks = p.get('tracks', [])
                for t in tracks:
                    tid = t.get('sid', '?')
                    ttype = t.get('type', '?')
                    mime = t.get('mimeType', '?')
                    src = t.get('source', '?')
                    print(f"  {identity}: track={tid} type={ttype} mime={mime} src={src}", flush=True)
        except:
            print("Raw:", out3[:2000], flush=True)
    else:
        print("(empty)", flush=True)

    # Check recent ohos events in docker logs with larger window
    cmd4 = "echo Tdcarefor123 | sudo -S docker logs --tail 5000 livekit-livekit-1 2>&1 | grep -E 'ohos_746' | tail -10"
    s4, o4, e4 = ssh.exec_command(cmd4, timeout=15)
    print("\n=== Recent ohos_746 events ===", flush=True)
    out4 = o4.read().decode('utf-8', errors='replace')
    print(out4[:2000] if out4 else "(none)", flush=True)

    ssh.close()
except Exception as ex:
    print(f"ERROR: {ex}", flush=True)
    import traceback
    traceback.print_exc()
    sys.exit(1)
