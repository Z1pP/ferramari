param(
    [int]$Monitor = 0
)

Add-Type -AssemblyName System.Windows.Forms

$screens = [System.Windows.Forms.Screen]::AllScreens
if ($Monitor -lt 0 -or $Monitor -ge $screens.Count) {
    Write-Error "monitor index $Monitor is outside 0..$($screens.Count - 1)"
    exit 2
}
$bounds = $screens[$Monitor].Bounds

$form = New-Object System.Windows.Forms.Form
$form.Text = "Ferramari benchmark cover"
$form.FormBorderStyle = [System.Windows.Forms.FormBorderStyle]::None
$form.StartPosition = [System.Windows.Forms.FormStartPosition]::Manual
$form.Bounds = $bounds
$form.BackColor = [System.Drawing.Color]::Black
$form.TopMost = $true
$form.ShowInTaskbar = $false
$form.Add_Shown({ $form.Activate() })
[System.Windows.Forms.Application]::Run($form)
