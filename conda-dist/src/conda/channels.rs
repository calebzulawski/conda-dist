use anyhow::{Context, Result};
use rattler_conda_types::{Channel, ChannelConfig};

pub const DEFAULT_CHANNEL: &str = "conda-forge";

pub fn parse_channels(channel_strings: &[String], config: &ChannelConfig) -> Result<Vec<Channel>> {
    channel_strings
        .iter()
        .map(|ch| {
            Channel::from_str(ch, config).with_context(|| format!("failed to parse channel '{ch}'"))
        })
        .collect()
}
