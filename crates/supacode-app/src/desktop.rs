//! Notifications du système (centre de notifications de Windows).
//!
//! SupaCode n'est pas une application empaquetée : il emprunte l'identité de PowerShell,
//! toujours enregistrée, pour afficher la notification sans installation préalable.

/// Lance la commande sans ouvrir de console (sous Windows, un programme console lancé
/// depuis une application fenêtrée ouvre sinon une fenêtre CMD le temps de son exécution).
pub fn no_window(command: &mut std::process::Command) -> &mut std::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command
}

/// Ouvre un fichier ou un dossier avec l'application associée, sans console.
pub fn open_path(path: &std::path::Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        no_window(std::process::Command::new("cmd").args(["/c", "start", ""]).arg(path)).spawn().map(drop)
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("xdg-open").arg(path).spawn().map(drop)
    }
}

/// Affiche une notification ; ne bloque pas et ignore les échecs.
pub fn notify(title: &str, body: &str) {
    #[cfg(windows)]
    {
        use base64::Engine as _;
        use std::os::windows::process::CommandExt;

        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const APP_ID: &str = r"{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe";
        let xml = format!(
            "<toast><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual></toast>",
            escape(title),
            escape(body)
        );
        let script = format!(
            "[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null\n\
             [Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime] | Out-Null\n\
             $xml = New-Object Windows.Data.Xml.Dom.XmlDocument\n\
             $xml.LoadXml('{}')\n\
             $toast = [Windows.UI.Notifications.ToastNotification]::new($xml)\n\
             [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('{}').Show($toast)\n",
            xml.replace('\'', "''"),
            APP_ID
        );
        // PowerShell attend le script encodé en UTF-16 LE, puis en base64.
        let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let encoded = base64::engine::general_purpose::STANDARD.encode(utf16);
        let _ = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-EncodedCommand", &encoded])
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = (title, body);
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    #[test]
    fn echappe_le_xml() {
        assert_eq!(super::escape("a < b & \"c\""), "a &lt; b &amp; &quot;c&quot;");
    }
}
