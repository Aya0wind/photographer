$ErrorActionPreference = "SilentlyContinue"
Write-Output "=== All VID_054C devices ==="
Get-PnpDevice -PresentOnly | Where-Object { $_.InstanceId -match "VID_054C" } | ForEach-Object {
  $props = $_ | Get-PnpDeviceProperty -KeyName "DEVPKEY_Device_DriverDesc", "DEVPKEY_Device_Service", "DEVPKEY_Device_ProblemCode"
  $drvDesc = ($props | Where-Object KeyName -eq "DEVPKEY_Device_DriverDesc").Data
  $svc = ($props | Where-Object KeyName -eq "DEVPKEY_Device_Service").Data
  $problem = ($props | Where-Object KeyName -eq "DEVPKEY_Device_ProblemCode").Data
  "{0}`n  class={1} status={2} problem={3} driver={4} service={5}" -f $_.InstanceId, $_.Class, $_.Status, $problem, $drvDesc, $svc
}
Write-Output ""
Write-Output "=== libusbK class devices ==="
Get-PnpDevice -PresentOnly | Where-Object { $_.ClassName -match "libusbK" -or $_.FriendlyName -match "libusb" } | ForEach-Object { $_.InstanceId + " [" + $_.Class + "] " + $_.FriendlyName }
