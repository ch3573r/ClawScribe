use super::super::model::ParakeetModel;
use super::protocol::{read_frame, write_frame, Request, Response};
use std::io;
#[cfg(windows)]
use std::io::Read;

pub(super) fn run() -> i32 {
    let input = io::stdin();
    let output = io::stdout();
    let mut input = input.lock();
    let mut output = output.lock();
    #[cfg(windows)]
    {
        // The parent assigns the kill-on-close job before releasing the child.
        let mut handshake = [0u8; 3];
        if input.read_exact(&mut handshake).is_err() || handshake != *b"GO\n" {
            return 2;
        }
    }

    let mut model = None;
    loop {
        let (request, samples) = match read_frame::<Request>(&mut input) {
            Ok(Some(frame)) => frame,
            Ok(None) => return 0,
            Err(_) => return 2,
        };
        if !matches!(request, Request::Transcribe) && !samples.is_empty() {
            return 2;
        }
        let response = match request {
            Request::Load {
                model_dir,
                precision_suffix,
                use_directml,
            } => {
                // Release the old sessions before allocating their replacement.
                drop(model.take());
                match ParakeetModel::new(&model_dir, precision_suffix.as_deref(), use_directml) {
                    Ok(loaded) => {
                        model = Some(loaded);
                        Response::Loaded
                    }
                    // Fixed errors cannot accidentally include model/file/transcript contents.
                    Err(_) => Response::Error("Parakeet model could not be loaded".into()),
                }
            }
            Request::Transcribe => match model.as_mut() {
                Some(model) => match model.transcribe_samples_with_retry(samples, &|| false) {
                    Ok(result) => Response::Transcript {
                        text: result.text,
                        timestamps: result.timestamps,
                        tokens: result.tokens,
                    },
                    Err(_) => Response::Error("Parakeet transcription failed".into()),
                },
                None => Response::Error("No Parakeet model loaded".into()),
            },
            Request::Shutdown => return 0,
        };
        if write_frame(&mut output, &response, &[]).is_err() {
            return 2;
        }
    }
}
