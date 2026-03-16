[CmdletBinding()]
param(
    [string]$Subject = "CN=ZWG Terminal Test",
    [string]$Password = "changeit",
    [string]$OutputDir = ""
)

$ErrorActionPreference = "Stop"
if ([string]::IsNullOrWhiteSpace($OutputDir)) {
    $OutputDir = Join-Path $PSScriptRoot "certs"
}
New-Item $OutputDir -ItemType Directory -Force | Out-Null

$securePassword = ConvertTo-SecureString -String $Password -Force -AsPlainText
$cert = New-SelfSignedCertificate `
    -Type Custom `
    -Subject $Subject `
    -KeyUsage DigitalSignature `
    -KeyAlgorithm RSA `
    -KeyLength 4096 `
    -HashAlgorithm SHA256 `
    -CertStoreLocation "Cert:\CurrentUser\My" `
    -TextExtension @("2.5.29.37={text}1.3.6.1.5.5.7.3.3")

$pfxPath = Join-Path $OutputDir "ZWGTerminal-TestCert.pfx"
$cerPath = Join-Path $OutputDir "ZWGTerminal-TestCert.cer"

Export-PfxCertificate -Cert $cert -FilePath $pfxPath -Password $securePassword | Out-Null
Export-Certificate -Cert $cert -FilePath $cerPath | Out-Null
Import-Certificate -FilePath $cerPath -CertStoreLocation "Cert:\CurrentUser\TrustedPeople" | Out-Null

Write-Host "Created certificate:"
Write-Host "  PFX: $pfxPath"
Write-Host "  CER: $cerPath"
Write-Host "  Thumbprint: $($cert.Thumbprint)"
