# Sign HAP using OpenHarmony SDK's bundled default debug signing materials.
# Generates fresh app/profile keypairs and issues certificate chains signed by
# OpenHarmony Application CA from the bundled OpenHarmony.p12 (password: 123456).

$ErrorActionPreference = 'Stop'

$sdk        = "C:\Program Files\Huawei\DevEco Studio\sdk\default\openharmony\toolchains\lib"
$signTool   = "$sdk\hap-sign-tool.jar"
$ohP12      = "$sdk\OpenHarmony.p12"
$profileTpl = "$sdk\UnsgnedDebugProfileTemplate.json"
$java       = "C:\Program Files\Huawei\DevEco Studio\jbr\bin\java.exe"
$keytool    = "C:\Program Files\Huawei\DevEco Studio\jbr\bin\keytool.exe"
$hdc        = "C:\Users\tzw\AppData\Local\OpenHarmony\Sdk\20\toolchains\hdc.exe"

$root         = "d:\tdcare\livekit\rust-sdks\examples\ohos-livekit-app"
$workDir      = "$root\signature"
$hapDir       = "$root\entry\build\default\outputs\default"
$unsignedHap  = "$hapDir\entry-default-unsigned.hap"
$signedHap    = "$hapDir\entry-default-signed.hap"
$bundleName   = "com.livekit.ohos.demo"

$myKeyP12     = "$workDir\livekit-key.p12"
$myKeyPwd     = "123456"
$appKeyAlias  = "livekit-app-key"
$profKeyAlias = "livekit-profile-key"

New-Item -ItemType Directory -Force -Path $workDir | Out-Null

# ---- 1. Get device UDID ----
Write-Host "===== 1. Get device UDID =====" -ForegroundColor Cyan
$udid = (& $hdc shell bm get --udid 2>&1 | Out-String).Trim()
if ($udid -match '([0-9A-F]{40,})') { $udid = $matches[1] }
Write-Host "Device UDID: $udid"
if (-not $udid) { throw "Failed to get device UDID" }

# ---- 2. Export root CA + intermediate CAs from OpenHarmony.p12 ----
Write-Host "`n===== 2. Export OpenHarmony root/CA certs =====" -ForegroundColor Cyan
$appCaCert   = "$workDir\app-ca.cer"
$rootCaCert  = "$workDir\root-ca.cer"
& $keytool -exportcert -keystore $ohP12 -storepass $myKeyPwd -storetype PKCS12 `
  -alias "openharmony application ca" -rfc -file $appCaCert | Out-Null
& $keytool -exportcert -keystore $ohP12 -storepass $myKeyPwd -storetype PKCS12 `
  -alias "openharmony application root ca" -rfc -file $rootCaCert | Out-Null

# ---- 3. Generate our app + profile keypairs ----
Write-Host "`n===== 3. Generate fresh app + profile keypairs =====" -ForegroundColor Cyan
if (Test-Path $myKeyP12) { Remove-Item $myKeyP12 }

& $java -jar $signTool generate-keypair `
  -keyAlias $appKeyAlias -keyAlg ECC -keySize NIST-P-256 `
  -keystoreFile $myKeyP12 -keystorePwd $myKeyPwd -keyPwd $myKeyPwd
& $java -jar $signTool generate-keypair `
  -keyAlias $profKeyAlias -keyAlg ECC -keySize NIST-P-256 `
  -keystoreFile $myKeyP12 -keystorePwd $myKeyPwd -keyPwd $myKeyPwd

# ---- 4. Issue app cert (chain) signed by "openharmony application ca" ----
Write-Host "`n===== 4. Issue app cert chain =====" -ForegroundColor Cyan
$appCertChain = "$workDir\app-cert.pem"
& $java -jar $signTool generate-app-cert `
  -keyAlias $appKeyAlias -signAlg SHA256withECDSA `
  -issuer "C=CN, O=OpenHarmony, OU=OpenHarmony Team, CN=OpenHarmony Application CA" `
  -issuerKeyAlias "openharmony application ca" `
  -subject "C=CN, O=OpenHarmony, OU=OpenHarmony Team, CN=LiveKit OHOS Demo App" `
  -validity 3650 -keystoreFile $myKeyP12 -keystorePwd $myKeyPwd -keyPwd $myKeyPwd `
  -issuerKeystoreFile $ohP12 -issuerKeystorePwd $myKeyPwd -issuerKeyPwd $myKeyPwd `
  -subCaCertFile $appCaCert -rootCaCertFile $rootCaCert `
  -outForm certChain -outFile $appCertChain
if (-not (Test-Path $appCertChain)) { throw "Failed to generate app cert chain" }

# ---- 5. Issue profile cert (chain) signed by "openharmony application ca" ----
Write-Host "`n===== 5. Issue profile cert chain =====" -ForegroundColor Cyan
$profCertChain = "$workDir\profile-cert.pem"
& $java -jar $signTool generate-profile-cert `
  -keyAlias $profKeyAlias -signAlg SHA256withECDSA `
  -issuer "C=CN, O=OpenHarmony, OU=OpenHarmony Team, CN=OpenHarmony Application CA" `
  -issuerKeyAlias "openharmony application ca" `
  -subject "C=CN, O=OpenHarmony, OU=OpenHarmony Team, CN=LiveKit OHOS Demo Profile" `
  -validity 3650 -keystoreFile $myKeyP12 -keystorePwd $myKeyPwd -keyPwd $myKeyPwd `
  -issuerKeystoreFile $ohP12 -issuerKeystorePwd $myKeyPwd -issuerKeyPwd $myKeyPwd `
  -subCaCertFile $appCaCert -rootCaCertFile $rootCaCert `
  -outForm certChain -outFile $profCertChain
if (-not (Test-Path $profCertChain)) { throw "Failed to generate profile cert chain" }

# ---- 6. Build profile JSON (replace bundleName, UDID, dev cert) ----
Write-Host "`n===== 6. Build profile template =====" -ForegroundColor Cyan
$tpl = Get-Content $profileTpl -Raw | ConvertFrom-Json
$tpl.'bundle-info'.'bundle-name' = $bundleName
$tpl.'debug-info'.'device-ids'   = @($udid)
$tpl.validity.'not-before'       = 1577836800   # 2020-01-01
$tpl.validity.'not-after'        = 2145916800   # 2038-01-01
# Replace development-certificate with our app cert end-entity
$chainText = Get-Content $appCertChain -Raw
$endMarker = "-----END CERTIFICATE-----"
$endIdx    = $chainText.IndexOf($endMarker)
$appEndEntity = $chainText.Substring(0, $endIdx + $endMarker.Length) + "`n"
# Normalize line endings to LF
$appEndEntity = $appEndEntity -replace "`r`n", "`n"
$tpl.'bundle-info'.'development-certificate' = $appEndEntity
$unsignedProfile = "$workDir\profile-unsigned.json"
$tpl | ConvertTo-Json -Depth 10 | Set-Content -Path $unsignedProfile -Encoding UTF8

# ---- 7. Sign profile ----
Write-Host "`n===== 7. Sign profile =====" -ForegroundColor Cyan
$signedProfile = "$workDir\profile.p7b"
if (Test-Path $signedProfile) { Remove-Item $signedProfile }
& $java -jar $signTool sign-profile `
  -keyAlias $profKeyAlias -signAlg SHA256withECDSA -mode localSign `
  -profileCertFile $profCertChain -inFile $unsignedProfile `
  -keystoreFile $myKeyP12 -outFile $signedProfile `
  -keyPwd $myKeyPwd -keystorePwd $myKeyPwd
if (-not (Test-Path $signedProfile)) { throw "Failed to sign profile" }

# ---- 8. Sign HAP ----
Write-Host "`n===== 8. Sign HAP =====" -ForegroundColor Cyan
if (Test-Path $signedHap) { Remove-Item $signedHap }
& $java -jar $signTool sign-app `
  -keyAlias $appKeyAlias -signAlg SHA256withECDSA -mode localSign `
  -appCertFile $appCertChain -profileFile $signedProfile `
  -inFile $unsignedHap -keystoreFile $myKeyP12 -outFile $signedHap `
  -keyPwd $myKeyPwd -keystorePwd $myKeyPwd -signCode 1
if (-not (Test-Path $signedHap)) { throw "Failed to sign HAP" }
Write-Host "Signed HAP -> $signedHap"

# ---- 9. Install ----
Write-Host "`n===== 9. Install signed HAP =====" -ForegroundColor Cyan
& $hdc install -r $signedHap
