Add-Type -AssemblyName System.Security

$pwdFile = 'C:\Users\tzw\AppData\Roaming\Huawei\DevEcoStudio6.0\c.pwd'
$content = Get-Content $pwdFile -Raw
$lines = $content -split "`n"
$masterKey = $null

foreach ($line in $lines) {
    if ($line -match '^value: !!binary (.+)$') {
        $b64 = $Matches[1].Trim()
        $bytes = [Convert]::FromBase64String($b64)
        $masterKey = [System.Security.Cryptography.ProtectedData]::Unprotect($bytes, $null, [System.Security.Cryptography.DataProtectionScope]::CurrentUser)
    }
}

function Try-Decrypt($hexPwd, $key, $label) {
    Write-Host "`n=== $label ==="
    $allBytes = [byte[]]::new($hexPwd.Length / 2)
    for ($i = 0; $i -lt $hexPwd.Length; $i += 2) {
        $allBytes[$i/2] = [Convert]::ToByte($hexPwd.Substring($i, 2), 16)
    }
    $encData = $allBytes[4..($allBytes.Length-1)]
    
    # Try AES-CTR with SHA256 of key and zero IV
    try {
        $sha = [System.Security.Cryptography.SHA256]::Create()
        $aesKeyBytes = $sha.ComputeHash($key)
        $aes = [System.Security.Cryptography.Aes]::Create()
        $aes.Key = $aesKeyBytes
        $aes.Mode = 'ECB'
        $aes.Padding = 'None'
        # CTR mode: encrypt counter to get keystream, XOR with ciphertext
        $counter = [byte[]]::new(16) # zero IV
        $keystream = [byte[]]::new($encData.Length + 16)
        $enc = $aes.CreateEncryptor()
        for ($i = 0; $i -lt $keystream.Length; $i += 16) {
            $block = $enc.TransformFinalBlock($counter, 0, 16)
            [Array]::Copy($block, 0, $keystream, $i, [Math]::Min(16, $keystream.Length - $i))
            # Increment counter (big-endian)
            for ($j = 15; $j -ge 0; $j--) {
                $counter[$j]++
                if ($counter[$j] -ne 0) { break }
            }
        }
        $decrypted = [byte[]]::new($encData.Length)
        for ($i = 0; $i -lt $encData.Length; $i++) {
            $decrypted[$i] = $encData[$i] -bxor $keystream[$i]
        }
        $r = [System.Text.Encoding]::UTF8.GetString($decrypted)
        Write-Host "CTR-SHA256: [$r]"
        # Check if result looks like a valid password (printable ASCII)
        $printable = $true
        foreach ($b in $decrypted) { if ($b -lt 32 -or $b -gt 126) { $printable = $false; break } }
        if ($printable -and $decrypted.Length -gt 0) {
            Write-Host "*** LIKELY CORRECT: $r ***"
            return $r
        }
    } catch { Write-Host "CTR-SHA256: $($_.Exception.Message)" }

    # Try AES-CBC with first 16 bytes of SHA256(key) as both key and IV
    try {
        $sha2 = [System.Security.Cryptography.SHA256]::Create()
        $k2 = $sha2.ComputeHash($key)
        $aes2 = [System.Security.Cryptography.Aes]::Create()
        $aes2.Key = $k2[0..15]
        $aes2.IV = $k2[16..31]
        $aes2.Mode = 'CBC'
        $aes2.Padding = 'PKCS7'
        $dec2 = $aes2.CreateDecryptor().TransformFinalBlock($encData, 0, $encData.Length)
        $r2 = [System.Text.Encoding]::UTF8.GetString($dec2)
        Write-Host "CBC-SHA256-both: [$r2]"
        $printable2 = $true
        foreach ($b in $dec2) { if ($b -lt 32 -or $b -gt 126) { $printable2 = $false; break } }
        if ($printable2 -and $dec2.Length -gt 0) {
            Write-Host "*** LIKELY CORRECT: $r2 ***"
            return $r2
        }
    } catch { Write-Host "CBC-SHA256-both: $($_.Exception.Message)" }

    # Try AES-CBC with key[0..15] as key and key[16..31] as IV  
    try {
        $aes3 = [System.Security.Cryptography.Aes]::Create()
        $aes3.Key = $key[0..15]
        $aes3.IV = $key[16..31]
        $aes3.Mode = 'CBC'
        $aes3.Padding = 'PKCS7'
        $dec3 = $aes3.CreateDecryptor().TransformFinalBlock($encData, 0, $encData.Length)
        $r3 = [System.Text.Encoding]::UTF8.GetString($dec3)
        Write-Host "CBC-raw: [$r3]"
    } catch { Write-Host "CBC-raw: $($_.Exception.Message)" }

    return $null
}

$sp = "0000001B860F9ABE178198CD6DCE609C93168309E8401B76F6223248C413DD2B9C478ECF2B382D9442FAB1"
$kp = "0000001BFB7D08FF6BB8B95AAAD52E535A6B72581CAC7A58EB2103E42D1C524552F6FA21E9A130403B874A"

$storeResult = Try-Decrypt $sp $masterKey "Store Password"
$keyResult = Try-Decrypt $kp $masterKey "Key Password"

if ($storeResult) {
    Write-Host "`nStore password: $storeResult"
}
if ($keyResult) {
    Write-Host "Key password: $keyResult"
}
