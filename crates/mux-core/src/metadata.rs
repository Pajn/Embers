use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Timestamp(pub SystemTime);

impl Timestamp {
    pub fn now() -> Self {
        Self(SystemTime::now())
    }
}

impl Default for Timestamp {
    fn default() -> Self {
        Self::now()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ActivityState {
    #[default]
    Idle,
    Activity,
    Bell,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntityMetadata {
    pub title: Option<String>,
    pub cwd: Option<PathBuf>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl EntityMetadata {
    pub fn new(title: Option<String>, cwd: Option<PathBuf>) -> Self {
        let now = Timestamp::now();
        Self {
            title,
            cwd,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn touch(&mut self) {
        self.updated_at = Timestamp::now();
    }
}

impl Default for EntityMetadata {
    fn default() -> Self {
        Self::new(None, None)
    }
}

#[cfg(test)]
mod tests {
    use super::EntityMetadata;

    #[test]
    fn touch_updates_timestamp() {
        let mut metadata = EntityMetadata::default();
        let first_updated_at = metadata.updated_at;

        metadata.touch();

        assert!(metadata.updated_at >= first_updated_at);
    }
}
