use anyhow::Result;
use rattler::default_cache_dir;
use rattler_repodata_gateway::{Gateway, GatewayBuilder};

pub fn build_gateway() -> Result<Gateway> {
    let mut builder = GatewayBuilder::new();
    #[cfg(not(target_arch = "wasm32"))]
    {
        let cache_root = default_cache_dir()?.join("repodata");
        builder.set_cache_dir(&cache_root);
    }

    Ok(builder.finish())
}
