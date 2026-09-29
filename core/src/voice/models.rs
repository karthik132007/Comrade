/**
 * Model manager: download-once model packs under `<comrade-agent>/models/`,
 * then fully offline inference. Never commit models to git.
 *
 * Layout:
 * ```text
 * models/
 *   manifest.json
 *   stt/encoder-epoch-99-avg-1.int8.onnx, decoder-*.int8.onnx,
 *       joiner-*.int8.onnx, tokens.txt (+ test/0.wav, test/trans.txt)
 *   vad/silero_vad.onnx
 *   tts/<kokoro files…> (+ .extracted marker)
 * ```
 */
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const MODELS_DIR: &str = "models";

pub struct ModelFile {
    pub path: &'static str,
    pub url: &'static str,
}

pub struct ModelPack {
    pub id: &'static str,
    pub dir: &'static str,
    pub files: &'static [ModelFile],
    pub archive: Option<ModelFile>,
}

const HF_STT: &str = "https://huggingface.co/csukuangfj/sherpa-onnx-streaming-zipformer-en-20M-2023-02-17/resolve/main";
const GITHUB_ASR: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models";
const GITHUB_TTS: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models";

const STT_FILES: &[ModelFile] = &[
    ModelFile { path: "encoder-epoch-99-avg-1.int8.onnx", url: "https://huggingface.co/csukuangfj/sherpa-onnx-streaming-zipformer-en-20M-2023-02-17/resolve/main/encoder-epoch-99-avg-1.int8.onnx" },
    ModelFile { path: "decoder-epoch-99-avg-1.int8.onnx", url: "https://huggingface.co/csukuangfj/sherpa-onnx-streaming-zipformer-en-20M-2023-02-17/resolve/main/decoder-epoch-99-avg-1.int8.onnx" },
    ModelFile { path: "joiner-epoch-99-avg-1.int8.onnx", url: "https://huggingface.co/csukuangfj/sherpa-onnx-streaming-zipformer-en-20M-2023-02-17/resolve/main/joiner-epoch-99-avg-1.int8.onnx" },
    ModelFile { path: "tokens.txt", url: "https://huggingface.co/csukuangfj/sherpa-onnx-streaming-zipformer-en-20M-2023-02-17/resolve/main/tokens.txt" },
    ModelFile { path: "test/0.wav", url: "https://huggingface.co/csukuangfj/sherpa-onnx-streaming-zipformer-en-20M-2023-02-17/resolve/main/test_wavs/0.wav" },
    ModelFile { path: "test/trans.txt", url: "https://huggingface.co/csukuangfj/sherpa-onnx-streaming-zipformer-en-20M-2023-02-17/resolve/main/test_wavs/trans.txt" },
];

const VAD_FILES: &[ModelFile] = &[
    ModelFile { path: "silero_vad.onnx", url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx" },
];

pub fn required_packs() -> Vec<ModelPack> {
    let _ = (HF_STT, GITHUB_ASR, GITHUB_TTS);
    vec![
        ModelPack { id: "stt", dir: "stt", files: STT_FILES, archive: None },
        ModelPack { id: "vad", dir: "vad", files: VAD_FILES, archive: None },
        ModelPack {
            id: "tts",
            dir: "tts",
            files: &[],
            archive: Some(ModelFile {
                path: "kokoro-int8-multi-lang-v1_1.tar.bz2",
                url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/kokoro-int8-multi-lang-v1_1.tar.bz2",
            }),
        },
    ]
}

/// Files a pack needs on disk to count as ready (archive expands to these).
pub fn tts_expected_files() -> Vec<String> {
    ["model.onnx", "voices.bin", "tokens.txt"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

pub fn models_dir() -> PathBuf {
    crate::paths::comrade_home().join(MODELS_DIR)
}

pub fn pack_dir(base: &Path, pack: &ModelPack) -> PathBuf {
    base.join(pack.dir)
}

/// Which files of a pack are missing.
pub fn missing_files(base: &Path, pack: &ModelPack) -> Vec<String> {
    let dir = pack_dir(base, pack);
    let mut missing: Vec<String> = pack
        .files
        .iter()
        .filter(|f| !dir.join(f.path).is_file())
        .map(|f| f.path.to_string())
        .collect();
    if pack.archive.is_some() {
        let extracted = dir.join(".extracted");
        let mut ok = extracted.is_file();
        if ok {
            for name in tts_expected_files() {
                if !dir.join(&name).is_file() {
                    ok = false;
                    break;
                }
            }
        }
        if !ok {
            let archive = pack.archive.as_ref().unwrap();
            let partial = partial_download_path(&dir.join(archive.path));
            let saved = std::fs::metadata(partial).map(|m| m.len()).unwrap_or(0);
            if saved > 0 {
                missing.push(format!(
                    "Kokoro TTS incomplete ({:.1} MB saved; Download will resume)",
                    saved as f64 / 1_048_576.0
                ));
            } else {
                missing.push("Kokoro TTS not downloaded".to_string());
            }
        }
    }
    missing
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum PackStatus {
    Ready,
    Missing { files: Vec<String> },
}

pub fn pack_status(base: &Path, pack: &ModelPack) -> PackStatus {
    let missing = missing_files(base, pack);
    if missing.is_empty() {
        PackStatus::Ready
    } else {
        PackStatus::Missing { files: missing }
    }
}

pub fn all_ready(base: &Path) -> bool {
    required_packs().iter().all(|p| matches!(pack_status(base, p), PackStatus::Ready))
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Manifest {
    pub files: HashMap<String, FileRecord>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileRecord {
    pub bytes: u64,
    pub sha256: String,
}

fn manifest_path(base: &Path) -> PathBuf {
    base.join("manifest.json")
}

pub fn load_manifest(base: &Path) -> Manifest {
    std::fs::read_to_string(manifest_path(base))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save_manifest(base: &Path, manifest: &Manifest) -> anyhow::Result<()> {
    std::fs::create_dir_all(base)?;
    let path = manifest_path(base);
    let pending = base.join("manifest.json.part");
    std::fs::write(&pending, serde_json::to_string_pretty(manifest)?)?;
    #[cfg(target_os = "windows")]
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    std::fs::rename(&pending, &path)?;
    Ok(())
}

#[derive(Clone, Debug)]
pub struct DownloadProgress {
    pub pack: String,
    pub file: String,
    pub downloaded: u64,
    pub total: Option<u64>,
}

fn partial_download_path(dest: &Path) -> PathBuf {
    let mut path = dest.as_os_str().to_owned();
    path.push(".part");
    PathBuf::from(path)
}

fn partial_content_total(value: &str, expected_start: u64) -> Option<u64> {
    let range = value.strip_prefix("bytes ")?;
    let (span, total) = range.split_once('/')?;
    let (start, end) = span.split_once('-')?;
    let start = start.parse::<u64>().ok()?;
    let end = end.parse::<u64>().ok()?;
    let total = total.parse::<u64>().ok()?;
    (start == expected_start && start <= end && end < total).then_some(total)
}

/// Download one file with progress. A partial transfer is kept separately and
/// resumed with HTTP Range when the server supports it. Returns (bytes, sha256).
async fn download_file(
    client: &reqwest::Client,
    file: &ModelFile,
    dest: &Path,
    progress: &(dyn Fn(DownloadProgress) + Send + Sync),
) -> anyhow::Result<(u64, String)> {
    use sha2::Digest;
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let pending = partial_download_path(dest);
    let existing = tokio::fs::metadata(&pending).await.map(|m| m.len()).unwrap_or(0);
    let mut request = client.get(file.url);
    if existing > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={existing}-"));
    }
    let mut res = request.send().await.map_err(|e| {
        anyhow::anyhow!("model download failed for {}: {e}", file.path)
    })?;
    if existing > 0 && res.status() == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
        tokio::fs::remove_file(&pending).await?;
        res = client.get(file.url).send().await.map_err(|e| {
            anyhow::anyhow!("model download failed for {}: {e}", file.path)
        })?;
    }
    if !res.status().is_success() {
        anyhow::bail!("model download failed for {}: HTTP {}", file.path, res.status());
    }
    let resume_total = if existing > 0 && res.status() == reqwest::StatusCode::PARTIAL_CONTENT {
        res.headers().get(reqwest::header::CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| partial_content_total(v, existing))
    } else {
        None
    };
    if existing > 0 && res.status() == reqwest::StatusCode::PARTIAL_CONTENT && resume_total.is_none() {
        anyhow::bail!("model download returned an invalid Content-Range for {}", file.path);
    }
    let resumed = resume_total.is_some();
    let total = if resumed { resume_total } else { res.content_length() };
    let mut hasher = sha2::Sha256::new();
    let mut downloaded = if resumed { existing } else { 0 };
    if resumed {
        use tokio::io::AsyncReadExt;
        let mut previous = tokio::fs::File::open(&pending).await?;
        let mut buf = [0u8; 64 * 1024];
        loop {
            let n = previous.read(&mut buf).await?;
            if n == 0 { break; }
            hasher.update(&buf[..n]);
        }
        progress(DownloadProgress {
            pack: String::new(), file: file.path.to_string(), downloaded, total,
        });
    }
    let mut out = tokio::fs::OpenOptions::new()
        .create(true).write(true).append(resumed).truncate(!resumed)
        .open(&pending).await?;
    let mut stream = res.bytes_stream();
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| anyhow::anyhow!("model download interrupted for {}: {e}", file.path))?;
        hasher.update(&chunk);
        out.write_all(&chunk).await?;
        downloaded += chunk.len() as u64;
        progress(DownloadProgress {
            pack: String::new(),
            file: file.path.to_string(),
            downloaded,
            total,
        });
    }
    out.flush().await?;
    drop(out);
    if let Some(expected) = total {
        if downloaded != expected {
            anyhow::bail!(
                "model download incomplete for {}: got {downloaded}, want {expected}",
                file.path
            );
        }
    }
    // A completed download becomes visible atomically, so model checks never
    // mistake a partial file for a usable model.
    if dest.is_file() {
        tokio::fs::remove_file(dest).await?;
    }
    tokio::fs::rename(&pending, dest).await?;
    Ok((downloaded, format!("{:x}", hasher.finalize())))
}

/// Ensure every pack is on disk, downloading what's missing. Progress events
/// carry pack ids; the returned manifest maps relative paths to records.
pub async fn ensure_models(
    base: &Path,
    progress: &(dyn Fn(DownloadProgress) + Send + Sync),
) -> anyhow::Result<Manifest> {
    let client = reqwest::Client::builder()
        .user_agent("Comrade voice-model-manager")
        .timeout(std::time::Duration::from_secs(600))
        .build()?;
    let mut manifest = load_manifest(base);
    for pack in required_packs() {
        let dir = pack_dir(base, &pack);
        std::fs::create_dir_all(&dir)?;
        for file in pack.files {
            let dest = dir.join(file.path);
            let key = format!("{}/{}", pack.dir, file.path);
            let recorded = manifest.files.get(&key).map(|r| r.bytes).unwrap_or(0);
            let actual = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
            if recorded > 0 && actual == recorded {
                continue; // verified present
            }
            let mut last_err = String::new();
            for attempt in 1..=3u32 {
                match download_file(&client, file, &dest, &|mut p: DownloadProgress| {
                    p.pack = pack.id.to_string();
                    progress(p);
                })
                .await
                {
                    Ok((bytes, sha256)) => {
                        manifest.files.insert(key.clone(), FileRecord { bytes, sha256 });
                        // Checkpoint completed files so a later pack failure does
                        // not make the next attempt download them all again.
                        save_manifest(base, &manifest)?;
                        last_err.clear();
                        break;
                    }
                    Err(e) => {
                        last_err = format!("{e}");
                        let _ = std::fs::remove_file(&dest); // never keep partials
                        crate::logger::log(
                            crate::logger::Level::Warn,
                            "VOICE",
                            "model download retry",
                            Some(&serde_json::json!({
                                "pack": pack.id, "file": file.path,
                                "attempt": attempt, "error": last_err.chars().take(160).collect::<String>(),
                            })),
                        );
                        tokio::time::sleep(std::time::Duration::from_secs(2 * attempt as u64)).await;
                    }
                }
            }
            if !last_err.is_empty() {
                anyhow::bail!("model download failed for {} after 3 tries: {last_err}", file.path);
            }
        }
        if let Some(ref archive) = pack.archive {
            extract_tts_archive(base, &pack, archive, progress).await?;
        }
    }
    save_manifest(base, &manifest)?;
    Ok(manifest)
}

/// Extract the Kokoro tarball (idempotent via `.extracted` marker).
async fn extract_tts_archive(
    base: &Path,
    pack: &ModelPack,
    archive: &ModelFile,
    progress: &(dyn Fn(DownloadProgress) + Send + Sync),
) -> anyhow::Result<()> {
    let dir = pack_dir(base, pack);
    let marker = dir.join(".extracted");
    let mut complete = marker.is_file();
    if complete {
        for name in tts_expected_files() {
            if !dir.join(&name).is_file() {
                complete = false;
                break;
            }
        }
    }
    if complete {
        return Ok(());
    }
    let dest = dir.join(archive.path);
    if !dest.is_file() {
        let client = reqwest::Client::builder()
            .user_agent("Comrade voice-model-manager")
            .connect_timeout(std::time::Duration::from_secs(20))
            .read_timeout(std::time::Duration::from_secs(60))
            .build()?;
        let mut last_err = String::new();
        for attempt in 1..=3u32 {
            match download_file(&client, archive, &dest, &|mut p: DownloadProgress| {
                p.pack = pack.id.to_string();
                progress(p);
            })
            .await
            {
                Ok(_) => {
                    last_err.clear();
                    break;
                }
                Err(e) => {
                    last_err = format!("{e}");
                    let _ = std::fs::remove_file(&dest);
                    tokio::time::sleep(std::time::Duration::from_secs(2 * attempt as u64)).await;
                }
            }
        }
        if !last_err.is_empty() {
            anyhow::bail!("model download failed for {} after 3 tries: {last_err}", archive.path);
        }
    }
    progress(DownloadProgress {
        pack: pack.id.to_string(),
        file: "extracting".to_string(),
        downloaded: 0,
        total: None,
    });
    // Decompress off the async executor and stream from disk instead of
    // holding the entire archive in memory.
    let dir_clone = dir.clone();
    let dest_clone = dest.clone();
    let extracted = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let raw = std::fs::File::open(&dest_clone)?;
        let bz = bzip2::read::BzDecoder::new(raw);
        let mut ar = tar::Archive::new(bz);
        ar.unpack(&dir_clone)?;
        Ok(())
    })
    .await
    .map_err(|e| anyhow::anyhow!("tts extract task failed: {e}"))?;
    if let Err(e) = extracted {
        // Force a clean re-download on the next attempt if a cached archive is
        // corrupt or was produced by an older interrupted version.
        let _ = std::fs::remove_file(&dest);
        return Err(e);
    }
    let _ = std::fs::remove_file(&dest);
    // Normalize layout even when the archive is long gone (repairs old installs).
    flatten_single_subdir(&dir)?;
    for name in tts_expected_files() {
        if !dir.join(&name).is_file() {
            anyhow::bail!("tts asset missing after extract: {name}");
        }
    }
    std::fs::write(&marker, "ok")?;
    Ok(())
}

fn flatten_single_subdir(dir: &Path) -> anyhow::Result<()> {
    let entries: Vec<_> = std::fs::read_dir(dir)?
        .flatten()
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name != ".extracted" && !name.ends_with(".tar.bz2") && !name.ends_with(".tbz")
        })
        .collect();
    if entries.len() == 1 && entries[0].path().is_dir() {
        let sub = entries[0].path();
        for entry in std::fs::read_dir(&sub)?.flatten() {
            let target = dir.join(entry.file_name());
            if target.exists() {
                continue;
            }
            std::fs::rename(entry.path(), &target)?;
        }
        let _ = std::fs::remove_dir(&sub);
    }
    // Normalize known kokoro asset names (release may version them).
    let mut onnx: Vec<(String, u64)> = vec![];
    for entry in std::fs::read_dir(dir)?.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let lower = name.to_lowercase();
        if lower.ends_with(".onnx") {
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            onnx.push((name.clone(), size));
        }
        let target: Option<&str> =
            if lower == "voices.bin" || (lower.ends_with(".bin") && lower.contains("voice")) {
                Some("voices.bin")
            } else if lower == "tokens.txt" || (lower.ends_with(".txt") && lower.contains("token")) {
                Some("tokens.txt")
            } else {
                None
            };
        if let Some(t) = target {
            let dst = dir.join(t);
            if name != t && !dst.exists() {
                std::fs::rename(entry.path(), &dst)?;
            }
        }
    }
    // Biggest .onnx wins unless a kokoro-named one exists.
    onnx.sort_by_key(|(n, s)| (!n.to_lowercase().contains("kokoro"), std::cmp::Reverse(*s)));
    if let Some((name, _)) = onnx.first() {
        let dst = dir.join("model.onnx");
        if name != "model.onnx" && !dst.exists() {
            std::fs::rename(dir.join(name), &dst)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_tts_archive() -> Vec<u8> {
        let encoder = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::fast());
        let mut archive = tar::Builder::new(encoder);
        for (path, contents) in [
            ("kokoro-fixture/model.onnx", b"model".as_slice()),
            ("kokoro-fixture/voices.bin", b"voices".as_slice()),
            ("kokoro-fixture/tokens.txt", b"tokens".as_slice()),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_mode(0o644);
            header.set_size(contents.len() as u64);
            header.set_cksum();
            archive.append_data(&mut header, path, contents).unwrap();
        }
        let encoder = archive.into_inner().unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn status_reflects_missing_files() {
        let dir = std::env::temp_dir().join("comrade-models-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("stt")).unwrap();
        let packs = required_packs();
        let stt = packs.iter().find(|p| p.id == "stt").unwrap();
        assert!(matches!(pack_status(&dir, stt), PackStatus::Missing { .. }));
        // Fake one file present; still missing overall.
        std::fs::write(dir.join("stt/tokens.txt"), "x").unwrap();
        assert!(matches!(pack_status(&dir, stt), PackStatus::Missing { .. }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn status_identifies_an_interrupted_tts_download() {
        let dir = std::env::temp_dir().join(format!(
            "comrade-partial-status-test-{}", std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let pack = required_packs().into_iter().find(|p| p.id == "tts").unwrap();
        let archive = dir.join("tts").join(pack.archive.as_ref().unwrap().path);
        std::fs::create_dir_all(archive.parent().unwrap()).unwrap();
        std::fs::write(partial_download_path(&archive), vec![0u8; 1024]).unwrap();

        let missing = missing_files(&dir, &pack);
        assert_eq!(missing.len(), 1);
        assert!(missing[0].contains("incomplete"));
        assert!(missing[0].contains("resume"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn manifest_round_trip() {
        let dir = std::env::temp_dir().join("comrade-manifest-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut m = Manifest::default();
        m.files.insert("stt/tokens.txt".into(), FileRecord { bytes: 10, sha256: "abc".into() });
        save_manifest(&dir, &m).unwrap();
        m.files.get_mut("stt/tokens.txt").unwrap().bytes = 11;
        save_manifest(&dir, &m).unwrap();
        let back = load_manifest(&dir);
        assert_eq!(back.files["stt/tokens.txt"].bytes, 11);
        assert!(!dir.join("manifest.json.part").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn flatten_moves_nested_assets() {
        let dir = std::env::temp_dir().join("comrade-flatten-test");
        let _ = std::fs::remove_dir_all(&dir);
        let sub = dir.join("kokoro-v1");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("kokoro-v1_0.onnx"), "x").unwrap();
        flatten_single_subdir(&dir).unwrap();
        assert!(dir.join("model.onnx").is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn fresh_tts_pack_downloads_and_extracts_archive() {
        use std::io::{Read, Write};

        let payload = fixture_tts_archive();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut request = [0u8; 2048];
            let _ = socket.read(&mut request);
            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                payload.len()
            );
            socket.write_all(headers.as_bytes()).unwrap();
            socket.write_all(&payload).unwrap();
        });

        let dir = std::env::temp_dir().join(format!(
            "comrade-tts-download-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let url: &'static str =
            Box::leak(format!("http://{address}/fixture.tar.bz2").into_boxed_str());
        let pack = ModelPack {
            id: "tts",
            dir: "tts",
            files: &[],
            archive: Some(ModelFile { path: "fixture.tar.bz2", url }),
        };
        let events = std::sync::Mutex::new(Vec::new());

        extract_tts_archive(
            &dir,
            &pack,
            pack.archive.as_ref().unwrap(),
            &|event| events.lock().unwrap().push(event),
        )
        .await
        .unwrap();
        server.join().unwrap();

        let tts_dir = dir.join("tts");
        assert!(tts_dir.join("model.onnx").is_file());
        assert!(tts_dir.join("voices.bin").is_file());
        assert!(tts_dir.join("tokens.txt").is_file());
        assert!(tts_dir.join(".extracted").is_file());
        assert!(!tts_dir.join("fixture.tar.bz2").exists());
        assert!(!tts_dir.join("fixture.tar.bz2.part").exists());
        assert!(missing_files(&dir, &pack).is_empty());

        let events = events.into_inner().unwrap();
        assert!(events.iter().any(|event| event.file == "fixture.tar.bz2"));
        assert!(events.iter().any(|event| event.file == "extracting"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn interrupted_download_resumes_without_restarting() {
        use sha2::Digest;
        use std::io::{Read, Write};

        let payload = fixture_tts_archive();
        let split = 17usize;
        let dir = std::env::temp_dir().join(format!(
            "comrade-resume-test-{}", std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("fixture.tar.bz2");
        std::fs::write(partial_download_path(&dest), &payload[..split]).unwrap();

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
            let mut request = [0u8; 2048];
            let n = socket.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..n]).to_lowercase();
            assert!(request.contains(&format!("range: bytes={split}-")), "{request}");
            let remaining = &payload[split..];
            let headers = format!(
                "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {split}-{}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                payload.len() - 1, payload.len(), remaining.len()
            );
            socket.write_all(headers.as_bytes()).unwrap();
            socket.write_all(remaining).unwrap();
            payload
        });
        let url: &'static str = Box::leak(format!("http://{address}/fixture.tar.bz2").into_boxed_str());
        let file = ModelFile { path: "fixture.tar.bz2", url };
        let events = std::sync::Mutex::new(Vec::new());
        let client = reqwest::Client::new();
        let (bytes, sha256) = download_file(&client, &file, &dest, &|event| {
            events.lock().unwrap().push(event);
        }).await.unwrap();
        let full = server.join().unwrap();
        assert_eq!(bytes, full.len() as u64);
        assert_eq!(std::fs::read(&dest).unwrap(), full);
        assert_eq!(sha256, format!("{:x}", sha2::Sha256::digest(&full)));
        assert_eq!(events.lock().unwrap().first().unwrap().downloaded, split as u64);
        assert!(!partial_download_path(&dest).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
