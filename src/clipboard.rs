use crate::settings::{AutoSubmitKey, ClipboardHandling, PasteMethod, Settings, TypingTool};
use log::info;
use std::process::Command;
use std::time::Duration;

pub fn output_text(
    text: &str,
    settings: &Settings,
) -> Result<(), String> {
    let paste_method = settings.paste_method();
    let clipboard_handling = settings.clipboard_handling();
    let paste_delay_ms = settings.paste_delay_ms();

    match paste_method {
        PasteMethod::Direct => type_text(text, settings),
        PasteMethod::CtrlV | PasteMethod::ShiftInsert | PasteMethod::CtrlShiftV => {
            paste_via_clipboard(text, paste_method, paste_delay_ms, clipboard_handling)
        }
        PasteMethod::None => {
            info!("Paste method is None, skipping output");
            Ok(())
        }
    }
}

fn type_text(text: &str, settings: &Settings) -> Result<(), String> {
    let typing_tool = settings.typing_tool();
    
    match typing_tool {
        TypingTool::Wtype => type_via_wtype(text),
        TypingTool::Kwtype => type_via_kwtype(text),
        TypingTool::Dotool => type_via_dotool(text),
        TypingTool::Ydotool => type_via_ydotool(text),
        TypingTool::Auto => {
            if is_wtype_available() {
                type_via_wtype(text)
            } else if is_ydotool_available() {
                type_via_ydotool(text)
            } else {
                Err("No typing tool available. Please install wtype or ydotool.".to_string())
            }
        }
    }
}

fn type_via_wtype(text: &str) -> Result<(), String> {
    let output = Command::new("wtype")
        .arg(text)
        .output()
        .map_err(|e| format!("Failed to run wtype: {}", e))?;

    if output.status.success() {
        Ok(())
    } else {
        Err(format!("wtype failed: {}", String::from_utf8_lossy(&output.stderr)))
    }
}

fn type_via_kwtype(text: &str) -> Result<(), String> {
    let output = Command::new("kwtype")
        .arg(text)
        .output()
        .map_err(|e| format!("Failed to run kwtype: {}", e))?;

    if output.status.success() {
        Ok(())
    } else {
        Err(format!("kwtype failed: {}", String::from_utf8_lossy(&output.stderr)))
    }
}

fn type_via_dotool(text: &str) -> Result<(), String> {
    let output = Command::new("dotool")
        .arg("type")
        .arg(text)
        .output()
        .map_err(|e| format!("Failed to run dotool: {}", e))?;

    if output.status.success() {
        Ok(())
    } else {
        Err(format!("dotool failed: {}", String::from_utf8_lossy(&output.stderr)))
    }
}

fn type_via_ydotool(text: &str) -> Result<(), String> {
    let output = Command::new("ydotool")
        .arg("type")
        .arg(text)
        .output()
        .map_err(|e| format!("Failed to run ydotool: {}", e))?;

    if output.status.success() {
        Ok(())
    } else {
        Err(format!("ydotool failed: {}", String::from_utf8_lossy(&output.stderr)))
    }
}

fn paste_via_clipboard(
    text: &str,
    paste_method: &PasteMethod,
    paste_delay_ms: u64,
    clipboard_handling: ClipboardHandling,
) -> Result<(), String> {
    write_clipboard_via_wl_copy(text)?;

    std::thread::sleep(Duration::from_millis(paste_delay_ms));

    send_paste_key_combo_wtype(paste_method)?;
    
    if clipboard_handling == ClipboardHandling::DontModify {
        std::thread::sleep(Duration::from_millis(100));
    }

    Ok(())
}

fn write_clipboard_via_wl_copy(text: &str) -> Result<(), String> {
    let mut child = Command::new("wl-copy")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to run wl-copy: {}", e))?;

    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        stdin.write_all(text.as_bytes()).map_err(|e| format!("Failed to write to wl-copy: {}", e))?;
    }

    let status = child.wait().map_err(|e| format!("Failed to wait for wl-copy: {}", e))?;
    
    if status.success() {
        Ok(())
    } else {
        Err("wl-copy failed".to_string())
    }
}

fn send_paste_key_combo_wtype(paste_method: &PasteMethod) -> Result<(), String> {
    match paste_method {
        PasteMethod::CtrlV => {
            Command::new("wtype")
                .args(["-M", "ctrl", "v", "-m", "ctrl"])
                .status()
                .map_err(|e| format!("Failed to send Ctrl+V: {}", e))?;
        }
        PasteMethod::ShiftInsert => {
            Command::new("wtype")
                .args(["-M", "shift", "-k", "Insert", "-m", "shift"])
                .status()
                .map_err(|e| format!("Failed to send Shift+Insert: {}", e))?;
        }
        PasteMethod::CtrlShiftV => {
            Command::new("wtype")
                .args(["-M", "ctrl", "-M", "shift", "v", "-m", "shift", "-m", "ctrl"])
                .status()
                .map_err(|e| format!("Failed to send Ctrl+Shift+V: {}", e))?;
        }
        _ => return Err("Unsupported paste method".to_string()),
    }
    Ok(())
}

pub fn press_auto_submit_key(auto_submit_key: AutoSubmitKey) -> Result<(), String> {
    match auto_submit_key {
        AutoSubmitKey::Enter => {
            Command::new("wtype")
                .args(["-k", "Return"])
                .status()
                .map_err(|e| format!("Failed to send Enter: {}", e))?;
        }
        AutoSubmitKey::CtrlEnter => {
            Command::new("wtype")
                .args(["-M", "ctrl", "-k", "Return", "-m", "ctrl"])
                .status()
                .map_err(|e| format!("Failed to send Ctrl+Enter: {}", e))?;
        }
        AutoSubmitKey::SuperEnter => {
            Command::new("wtype")
                .args(["-M", "super", "-k", "Return", "-m", "super"])
                .status()
                .map_err(|e| format!("Failed to send Super+Enter: {}", e))?;
        }
    }
    Ok(())
}

fn is_wtype_available() -> bool {
    Command::new("which")
        .arg("wtype")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn is_ydotool_available() -> bool {
    Command::new("which")
        .arg("ydotool")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn get_available_typing_tools() -> Vec<String> {
    let mut tools = vec!["auto".to_string()];
    
    if is_wtype_available() {
        tools.push("wtype".to_string());
    }
    if Command::new("which").arg("kwtype").output().map(|o| o.status.success()).unwrap_or(false) {
        tools.push("kwtype".to_string());
    }
    if Command::new("which").arg("dotool").output().map(|o| o.status.success()).unwrap_or(false) {
        tools.push("dotool".to_string());
    }
    if is_ydotool_available() {
        tools.push("ydotool".to_string());
    }
    
    tools
}
