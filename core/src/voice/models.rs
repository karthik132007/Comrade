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
            missing.push("<kokoro archive>".to_string());
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
    std::fs::write(manifest_path(base), serde_json::to_string_pretty(manifest)?)?;
    Ok(())
}

#[derive(Clone, Debug)]
pub struct DownloadProgress {
    pub pack: String,
    pub file: String,
    pub downloaded: u64,
    pub total: Option<u64>,
}

/// Download one file with progress. Returns (bytes, sha256).
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
    let res = client.get(file.url).send().await.map_err(|e| {
        anyhow::anyhow!("model download failed for {}: {e}", file.path)
    })?;
    if !res.status().is_success() {
        anyhow::bail!("model download failed for {}: HTTP {}", file.path, res.status());
    }
    let total = res.content_length();
    let mut hasher = sha2::Sha256::new();
    let mut out = tokio::fs::File::create(dest).await?;
    let mut downloaded = 0u64;
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
    if let Some(expected) = total {
        if downloaded != expected {
            anyhow::bail!(
                "model download incomplete for {}: got {downloaded}, want {expected}",
                file.path
            );
        }
    }
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
    if dest.is_file() {
        let client =
            reqwest::Client::builder().user_agent("Comrade voice-model-manager").build()?;
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
        progress(DownloadProgress {
            pack: pack.id.to_string(),
            file: "extracting".to_string(),
            downloaded: 0,
            total: None,
        });
        // Decompress off the async executor: bzip2 + tar are blocking.
        let dir_clone = dir.clone();
        let dest_clone = dest.clone();
        tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let raw = std::fs::read(&dest_clone)?;
            let cursor = std::io::Cursor::new(raw);
            let bz = bzip2::read::BzDecoder::new(cursor);
            let mut ar = tar::Archive::new(bz);
            ar.unpack(&dir_clone)?;
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("tts extract task failed: {e}"))??;
        let _ = std::fs::remove_file(&dest);
    }
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
    fn manifest_round_trip() {
        let dir = std::env::temp_dir().join("comrade-manifest-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut m = Manifest::default();
        m.files.insert("stt/tokens.txt".into(), FileRecord { bytes: 10, sha256: "abc".into() });
        save_manifest(&dir, &m).unwrap();
        let back = load_manifest(&dir);
        assert_eq!(back.files["stt/tokens.txt"].bytes, 10);
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
}
