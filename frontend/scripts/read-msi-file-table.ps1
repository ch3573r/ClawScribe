param([Parameter(Mandatory)][string]$Installer)

$ErrorActionPreference = "Stop"
$path = (Resolve-Path -LiteralPath $Installer).Path
$windowsInstaller = New-Object -ComObject WindowsInstaller.Installer
$database = $windowsInstaller.OpenDatabase($path, 0) # read-only
$view = $database.OpenView('SELECT ' +
    '`File`.`File`, `File`.`FileName`, `Component`.`Directory_` ' +
    'FROM `File`, `Component` WHERE `File`.`Component_` = `Component`.`Component`')
$null = $view.Execute()
try {
    while ($row = $view.Fetch()) {
        $key = $row.StringData(1)
        $name = ($row.StringData(2) -split '\|')[-1]
        $directory = $row.StringData(3)
        foreach ($segment in @($key, $name, $directory)) {
            if (-not $segment -or $segment -in @(".", "..") -or
                $segment.IndexOfAny([IO.Path]::GetInvalidFileNameChars()) -ge 0) {
                throw "The MSI file table contains an invalid path segment."
            }
        }
        [pscustomobject]@{ key = $key; name = $name; directory = $directory }
    }
} finally {
    $null = $view.Close()
}
