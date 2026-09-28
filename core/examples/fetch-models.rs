#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let base = comrade_core::voice::models::models_dir();
    println!("target: {}", base.display());
    comrade_core::voice::models::ensure_models(&base, &|p| {
        println!("  [{:<3}] {} {:.1}MB", p.pack, p.file, p.downloaded as f64 / 1048576.0);
    })
    .await?;
    println!("ALL MODELS READY");
    Ok(())
}
