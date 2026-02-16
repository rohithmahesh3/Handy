use crate::input::EnigoState;
use crate::settings::{AutoSubmitKey, ClipboardHandling, PasteMethod, Settings, TypingTool};
use enigo::{Direction, Enigo, Key, Keyboard};
use log::info;
use std::process::Command;
use std::time::Duration;

pub fn output_text(
    enigo: &mut Enigo,
    text: &str,
    settings: &Settings,
) -> Result<(), String> {
    let paste_method = settings.paste_method();
    let clipboard_handling = settings.clipboard_handling();
    let paste_delay_ms = settings.paste_delay_ms();

    match paste_method {
        PasteMethod::Direct => type_text(enigo, text, settings),
        PasteMethod::CtrlV | PasteMethod::ShiftInsert | PasteMethod::CtrlShiftV => {
            paste_via_clipboard(enigo, text, paste_method, paste_delay_ms, clipboard_handling)
        }
        PasteMethod::None => {
            info!("Paste method is None, skipping output");
            Ok(())
        }
    }
}

fn type_text(
    enigo: &mut Enigo,
    text: &str,
    settings: &Settings,
) -> Result<(), String> {
    let typing_tool = settings.typing_tool();
    
    match typing_tool {
        TypingTool::Wtype => type_via_wtype(text),
        TypingTool::Kwtype => type_via_kwtype(text),
        TypingTool::Dotool => type_via_dotool(text),
        TypingTool::Ydotool => type_via_ydotool(text),
        TypingTool::Xdotool => type_via_xdotool(text),
        TypingTool::Auto => {
            if is_wtype_available() {
                type_via_wtype(text)
            } else if is_ydotool_available() {
                type_via_ydotool(text)
            } else {
                type_via_enigo(enigo, text)
            }
        }
    }
}

fn type_via_enigo(enigo: &mut Enigo, text: &str) -> Result<(), String> {
    enigo.text(text).map_err(|e| format!("Failed to type text: {}", e))
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

fn type_via_xdotool(text: &str) -> Result<(), String> {
    let output = Command::new("xdotool")
        .arg("type")
        .arg("--clearmodifiers")
        .arg(text)
        .output()
        .map_err(|e| format!("Failed to run xdotool: {}", e))?;

    if output.status.success() {
        Ok(())
    } else {
        Err(format!("xdotool failed: {}", String::from_utf8_lossy(&output.stderr)))
    }
}

fn paste_via_clipboard(
    enigo: &mut Enigo,
    text: &str,
    paste_method: &PasteMethod,
    paste_delay_ms: u64,
    clipboard_handling: ClipboardHandling,
) -> Result<(), String> {
    write_clipboard_via_wl_copy(text)?;

    std::thread::sleep(Duration::from_millis(paste_delay_ms));

    let key_combo_sent = send_paste_key_combo(enigo, paste_method)?;
    
    if !key_combo_sent {
        return Err("Failed to send paste key combo".to_string());
    }

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

fn send_paste_key_combo(enigo: &mut Enigo, paste_method: &PasteMethod) -> Result<bool, String> {
    match paste_method {
        PasteMethod::CtrlV => {
            enigo.key(Key::Control, Direction::Press).map_err(|e| e.to_string())?;
            enigo.key(Key::Unicode('v'), Direction::Click).map_err(|e| e.to_string())?;
            enigo.key(Key::Control, Direction::Release).map_err(|e| e.to_string())?;
        }
        PasteMethod::ShiftInsert => {
            enigo.key(Key::Shift, Direction::Press).map_err(|e| e.to_string())?;
            enigo.key(Key::Insert, Direction::Click).map_err(|e| e.to_string())?;
            enigo.key(Key::Shift, Direction::Release).map_err(|e| e.to_string())?;
        }
        PasteMethod::CtrlShiftV => {
            enigo.key(Key::Control, Direction::Press).map_err(|e| e.to_string())?;
            enigo.key(Key::Shift, Direction::Press).map_err(|e| e.to_string())?;
            enigo.key(Key::Unicode('v'), Direction::Click).map_err(|e| e.to_string())?;
            enigo.key(Key::Shift, Direction::Release).map_err(|e| e.to_string())?;
            enigo.key(Key::Control, Direction::Release).map_err(|e| e.to_string())?;
        }
        _ => return Ok(false),
    }
    Ok(true)
}

pub fn press_auto_submit_key(enigo: &mut Enigo, auto_submit_key: AutoSubmitKey) -> Result<(), String> {
    match auto_submit_key {
        AutoSubmitKey::Enter => {
            enigo.key(Key::Return, Direction::Click).map_err(|e| e.to_string())?;
        }
        AutoSubmitKey::CtrlEnter => {
            enigo.key(Key::Control, Direction::Press).map_err(|e| e.to_string())?;
            enigo.key(Key::Return, Direction::Click).map_err(|e| e.to_string())?;
            enigo.key(Key::Control, Direction::Release).map_err(|e| e.to_string())?;
        }
        AutoSubmitKey::CmdEnter => {
            enigo.key(Key::Meta, Direction::Press).map_err(|e| e.to_string())?;
            enigo.key(Key::Return, Direction::Click).map_err(|e| e.to_string())?;
            enigo.key(Key::Meta, Direction::Release).map_err(|e| e.to_string())?;
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
    if Command::new("which").arg("xdotool").output().map(|o| o.status.success()).unwrap_or(false) {
        tools.push("xdotool".to_string());
    }
    
    tools
}
