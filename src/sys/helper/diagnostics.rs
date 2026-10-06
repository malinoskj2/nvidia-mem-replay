use super::HelperError;
use std::{
    io::{self, Read},
    process::ChildStderr,
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
};

pub(super) const MAX_DIAGNOSTIC_BYTES: usize = 8192;
const TRUNCATED_NOTICE: &str = "[helper stderr truncated after 8192 bytes]";

#[derive(Default)]
struct Capture {
    bytes: Vec<u8>,
    truncated: bool,
}

pub(super) struct Diagnostics {
    capture: Arc<Mutex<Capture>>,
    reader: Option<JoinHandle<()>>,
}

impl Diagnostics {
    pub(super) fn new(stderr: Option<ChildStderr>) -> Self {
        let capture = Arc::new(Mutex::new(Capture::default()));
        let output = Arc::clone(&capture);
        let reader = stderr.map(|mut stderr| {
            thread::spawn(move || {
                let mut buffer = [0; 1024];
                loop {
                    let count = match stderr.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(count) => count,
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    };

                    let Ok(mut capture) = output.lock() else {
                        break;
                    };
                    let retained = count.min(MAX_DIAGNOSTIC_BYTES - capture.bytes.len());
                    capture.bytes.extend_from_slice(&buffer[..retained]);
                    capture.truncated |= retained < count;
                    // Drain excess output so a full stderr pipe cannot block the helper.
                }
            })
        });

        Self { capture, reader }
    }

    pub(super) fn finish(&mut self) {
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }

    pub(super) fn attach(&self, error: HelperError) -> HelperError {
        let error = match error {
            HelperError::Diagnostics { error, .. } => *error,
            error => error,
        };

        let Ok(capture) = self.capture.lock() else {
            return error;
        };
        let text = String::from_utf8_lossy(&capture.bytes);
        let text = text.trim();
        if text.is_empty() && !capture.truncated {
            return error;
        }

        let diagnostics = if capture.truncated {
            format!("\n{text}\n{TRUNCATED_NOTICE}")
        } else {
            format!("\n{text}")
        };

        HelperError::Diagnostics {
            error: Box::new(error),
            diagnostics,
        }
    }
}
