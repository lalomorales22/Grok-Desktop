use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;

use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use tauri::{AppHandle, Emitter};

use crate::error::{AppError, AppResult};
use crate::types::{TerminalEvent, TerminalHandle};

pub struct TerminalSession {
    pub master: Mutex<Box<dyn MasterPty + Send>>,
    pub writer: Mutex<Box<dyn Write + Send>>,
    pub child: Mutex<Box<dyn portable_pty::Child + Send + Sync>>,
}

pub type TerminalRegistry = Mutex<HashMap<String, Arc<TerminalSession>>>;

pub async fn ensure_terminal(
    app: AppHandle,
    registry: &TerminalRegistry,
) -> AppResult<TerminalHandle> {
    if let Some(existing) = registry
        .lock()
        .map_err(|_| AppError::message("terminal registry lock poisoned"))?
        .keys()
        .next()
        .cloned()
    {
        return Ok(TerminalHandle {
            session_id: existing,
        });
    }

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 30,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| AppError::message(error.to_string()))?;

    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    let home_dir = dirs::home_dir().unwrap_or_else(std::env::temp_dir);
    let mut command = CommandBuilder::new(shell);
    command.arg("-l");
    command.arg("-i");
    command.cwd(&home_dir);
    command.env("HOME", home_dir.to_string_lossy().to_string());
    command.env("PATH", shell_path());
    command.env("TERM", "xterm-256color");
    command.env("CLICOLOR", "0");
    command.env("CLICOLOR_FORCE", "0");
    command.env("NO_COLOR", "1");

    let child = pair
        .slave
        .spawn_command(command)
        .map_err(|error| AppError::message(error.to_string()))?;
    drop(pair.slave);

    let master = pair.master;
    let reader = master
        .try_clone_reader()
        .map_err(|error| AppError::message(error.to_string()))?;
    let writer = master
        .take_writer()
        .map_err(|error| AppError::message(error.to_string()))?;

    let session_id = uuid::Uuid::new_v4().to_string();
    let session = Arc::new(TerminalSession {
        master: Mutex::new(master),
        writer: Mutex::new(writer),
        child: Mutex::new(child),
    });

    registry
        .lock()
        .map_err(|_| AppError::message("terminal registry lock poisoned"))?
        .insert(session_id.clone(), Arc::clone(&session));

    spawn_reader(app.clone(), session_id.clone(), reader);
    spawn_waiter(app, session_id.clone(), Arc::clone(&session));

    Ok(TerminalHandle { session_id })
}

pub async fn write_input(
    registry: &TerminalRegistry,
    session_id: &str,
    input: &str,
) -> AppResult<()> {
    let session = registry
        .lock()
        .map_err(|_| AppError::message("terminal registry lock poisoned"))?
        .get(session_id)
        .cloned()
        .ok_or_else(|| AppError::message("terminal session not found"))?;

    let mut writer = session
        .writer
        .lock()
        .map_err(|_| AppError::message("terminal writer lock poisoned"))?;
    writer.write_all(input.as_bytes())?;
    writer.flush()?;
    Ok(())
}

pub async fn terminate_terminal(registry: &TerminalRegistry, session_id: &str) -> AppResult<()> {
    let session = registry
        .lock()
        .map_err(|_| AppError::message("terminal registry lock poisoned"))?
        .get(session_id)
        .cloned()
        .ok_or_else(|| AppError::message("terminal session not found"))?;

    let mut child = session
        .child
        .lock()
        .map_err(|_| AppError::message("terminal child lock poisoned"))?;
    child.kill()?;
    Ok(())
}

pub async fn resize_terminal(
    registry: &TerminalRegistry,
    session_id: &str,
    cols: u16,
    rows: u16,
) -> AppResult<()> {
    let session = registry
        .lock()
        .map_err(|_| AppError::message("terminal registry lock poisoned"))?
        .get(session_id)
        .cloned()
        .ok_or_else(|| AppError::message("terminal session not found"))?;

    let master = session
        .master
        .lock()
        .map_err(|_| AppError::message("terminal master lock poisoned"))?;
    master
        .resize(PtySize {
            rows: rows.max(1),
            cols: cols.max(1),
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| AppError::message(error.to_string()))
}

fn spawn_reader(app: AppHandle, session_id: String, mut reader: Box<dyn Read + Send>) {
    thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    let chunk = String::from_utf8_lossy(&buffer[..read]).to_string();
                    let _ = app.emit(
                        "terminal://event",
                        TerminalEvent {
                            session_id: session_id.clone(),
                            kind: "output".to_string(),
                            chunk: Some(chunk),
                            stream: Some("stdout".to_string()),
                            exit_code: None,
                        },
                    );
                }
                Err(error) => {
                    let _ = app.emit(
                        "terminal://event",
                        TerminalEvent {
                            session_id: session_id.clone(),
                            kind: "output".to_string(),
                            chunk: Some(format!("\n[terminal read error: {error}]\n")),
                            stream: Some("stderr".to_string()),
                            exit_code: None,
                        },
                    );
                    break;
                }
            }
        }
    });
}

fn spawn_waiter(app: AppHandle, session_id: String, session: Arc<TerminalSession>) {
    thread::spawn(move || {
        let exit_code = {
            let mut child = match session.child.lock() {
                Ok(child) => child,
                Err(_) => return,
            };
            match child.wait() {
                Ok(status) => Some(status.exit_code() as i32),
                Err(_) => None,
            }
        };

        let _ = app.emit(
            "terminal://event",
            TerminalEvent {
                session_id: session_id.clone(),
                kind: "exit".to_string(),
                chunk: None,
                stream: None,
                exit_code,
            },
        );
    });
}

fn shell_path() -> String {
    let mut ordered = vec![
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/opt/homebrew/sbin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/local/sbin"),
        PathBuf::from("/opt/local/bin"),
        PathBuf::from("/opt/local/sbin"),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/bin"),
        PathBuf::from("/usr/sbin"),
        PathBuf::from("/sbin"),
    ];

    if let Some(existing) = std::env::var_os("PATH") {
        for path in std::env::split_paths(&existing) {
            if !ordered.iter().any(|candidate| candidate == &path) {
                ordered.push(path);
            }
        }
    }

    std::env::join_paths(ordered)
        .unwrap_or_default()
        .to_string_lossy()
        .to_string()
}
