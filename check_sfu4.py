import paramiko

ssh = paramiko.SSHClient()
ssh.set_missing_host_key_policy(paramiko.AutoAddPolicy())
ssh.connect('livekit.tdcare.cn', username='ubuntu', password='Tdcarefor123', timeout=10, look_for_keys=False, allow_agent=False)

# Search for sender reports with the new track IDs
cmd = "echo Tdcarefor123 | sudo -S docker logs --tail 10000 livekit-livekit-1 2>&1 | grep 'sender report' | grep -E 'TR_AMDmwCffY6bx6r|TR_VCeKRRmbRbtvjL|TR_VCHejbj7vXBRNP' | tail -10"
stdin, stdout, stderr = ssh.exec_command(cmd, timeout=20)
print("=== New track sender reports ===")
out = stdout.read().decode('utf-8', errors='replace')
if out.strip():
    print(out[:2000])
else:
    print("(none found — sender reports may take time to generate)")

# Also check all sender reports in tail 10000
cmd2 = "echo Tdcarefor123 | sudo -S docker logs --tail 10000 livekit-livekit-1 2>&1 | grep 'sender report' | tail -10"
stdin2, stdout2, stderr2 = ssh.exec_command(cmd2, timeout=20)
print("\n=== All sender reports (tail 10000) ===")
out2 = stdout2.read().decode('utf-8', errors='replace')
for line in out2.strip().split('\n')[-10:]:
    print(line[:300])

ssh.close()
