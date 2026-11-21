# Wait for the installed webview's idle recording control before sending native CLI intents.
param([Parameter(Mandatory)][int]$AppProcessId)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName UIAutomationClient
$start = [System.Windows.Automation.PropertyCondition]::new(
    [System.Windows.Automation.AutomationElement]::NameProperty, 'Start recording')
$enabled = [System.Windows.Automation.PropertyCondition]::new(
    [System.Windows.Automation.AutomationElement]::IsEnabledProperty, $true)
$button = [System.Windows.Automation.PropertyCondition]::new(
    [System.Windows.Automation.AutomationElement]::ControlTypeProperty, [System.Windows.Automation.ControlType]::Button)
$condition = [System.Windows.Automation.AndCondition]::new(
    [System.Windows.Automation.Condition[]]@($start, $enabled, $button))
$timer = [Diagnostics.Stopwatch]::StartNew()
while ($timer.Elapsed.TotalSeconds -lt 40) {
    $window = (Get-Process -Id $AppProcessId).MainWindowHandle
    if ($window -ne [IntPtr]::Zero) {
        try {
            $root = [System.Windows.Automation.AutomationElement]::FromHandle($window)
            if ($root.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $condition)) { exit 0 }
        } catch [System.Windows.Automation.ElementNotAvailableException] {
            # The webview may replace its initial document during startup.
        }
    }
    Start-Sleep -Milliseconds 250
}
exit 1
