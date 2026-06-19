#!/usr/bin/env python3
"""Check SFU logs around the full reconnect for session PA_kX9kCEcpcr6V"""
import paramiko, json

ssh = paramiko.SSHClient()
ssh.set_missing_host_key_policy(paramiko.AutoAddPolicy())
ssh.connect('livekit.tdcare.cn', username='ubuntu', password='Tdcarefor123', timeout=10, look_for_keys=False, allow_agent=False)

# Full reconnect and session 2 logs
cmd = "echo Tdcarefor123 | sudo -S docker logs --since 1h livekit-livekit-1 2>&1 | grep 'PA_kX9kCEcpcr6V' | tail -50"
stdin, stdout, stderr = ssh.exec_command(cmd, timeout=15)
print("=== Session 2 (PA_kX9kCEcpcr6V) ALL events ===")
for line in stdout.read().decode('utf-8', errors='replace').strip().split('\n')[-50:]:
    print(line[:400])

# Check for full reconnect log
cmd2 = "echo Tdcarefor123 | sudo -S docker logs --since 1h livekit-livekit-1 2>&1 | grep -i 'full reconnect' | tail -5"
stdin2, stdout2, stderr2 = ssh.exec_command(cmd2, timeout=15)
print("\n=== full reconnect events ===")
print(stdout2.read().decode('utf-8', errors='replace')[:3000])

ssh.close()
