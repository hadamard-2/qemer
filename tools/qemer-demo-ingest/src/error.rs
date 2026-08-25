#[derive(Debug, thiserror::Error)]
pub enum IngestError {
    #[error("the snapshot contains no exact 32-dash separator")]
    MissingSeparator,
    #[error("block {block} has no `### ` title")]
    MissingTitle { block: usize },
    #[error("block {block} has no `Source:` URL")]
    MissingSource { block: usize },
    #[error("block {block} has more than one `Source:` URL")]
    MultipleSources { block: usize },
    #[error("block {block} has an unterminated code fence")]
    UnterminatedFence { block: usize },
    #[error("block {block} has neither prose nor code")]
    EmptyBlock { block: usize },
    #[error("output directory {path} must not already exist")]
    OutputAlreadyExists { path: String },
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("embedding request failed: {0}")]
    Embed(String),
    #[error("Parquet output failed: {0}")]
    Parquet(String),
    #[error("archive output failed: {0}")]
    Archive(String),
    #[error("manifest output failed: {0}")]
    Manifest(String),
}
