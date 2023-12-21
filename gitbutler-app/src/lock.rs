use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub struct Dir {
    inner: Arc<Inner>,
}

impl Dir {
    pub fn new<P: AsRef<std::path::Path>>(path: P) -> Result<Self, OpenError> {
        Inner::new(path).map(Arc::new).map(|inner| Self { inner })
    }

    pub fn batch<R, E>(
        &self,
        action: impl FnOnce(&std::path::Path) -> Result<R, E>,
    ) -> Result<R, BatchError<E>> {
        self.inner.batch(action)
    }
}

#[derive(Debug)]
struct Inner {
    path: std::path::PathBuf,
    flock: Mutex<fslock::LockFile>,
}

impl Inner {
    fn new<P: AsRef<std::path::Path>>(path: P) -> Result<Self, OpenError> {
        let path = path.as_ref().to_path_buf();
        if !path.is_dir() {
            return Err(OpenError::NotDirectory(path));
        }
        let flock = fslock::LockFile::open(&path.with_extension("lock")).map(Mutex::new)?;
        Ok(Self { path, flock })
    }

    fn batch<R, E>(
        &self,
        action: impl FnOnce(&std::path::Path) -> Result<R, E>,
    ) -> Result<R, BatchError<E>> {
        let mut flock = self.flock.lock().unwrap();

        flock.lock()?;
        let result = action(&self.path).map_err(BatchError::Batch);
        flock.unlock()?;

        result
    }
}

#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    #[error("{0} is not a directory")]
    NotDirectory(std::path::PathBuf),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum BatchError<E> {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Batch(E),
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::test_utils::temp_dir;

    #[tokio::test]
    async fn test_lock_same_instance() {
        let dir_path = temp_dir();
        std::fs::write(dir_path.join("file.txt"), "").unwrap();
        let dir = Dir::new(&dir_path).unwrap();

        let (tx, rx) = std::sync::mpsc::sync_channel(1);

        // spawn a task that will signal right after aquireing the lock
        let _ = tokio::spawn({
            let dir = dir.clone();
            async move {
                dir.batch(|root| {
                    tx.send(()).unwrap();
                    assert_eq!(
                        std::fs::read_to_string(root.join("file.txt")).unwrap(),
                        String::new()
                    );
                    std::fs::write(root.join("file.txt"), "1")
                })
            }
        })
        .await
        .unwrap();

        // then we wait until the lock is aquired
        rx.recv().unwrap();

        // and immidiately try to lock again
        dir.batch(|root| {
            assert_eq!(std::fs::read_to_string(root.join("file.txt")).unwrap(), "1");
            std::fs::write(root.join("file.txt"), "2")
        })
        .unwrap();

        assert_eq!(
            std::fs::read_to_string(dir_path.join("file.txt")).unwrap(),
            "2"
        );
    }

    #[tokio::test]
    async fn test_lock_different_instances() {
        let dir_path = temp_dir();
        std::fs::write(dir_path.join("file.txt"), "").unwrap();

        let (tx, rx) = std::sync::mpsc::sync_channel(1);

        // spawn a task that will signal right after aquireing the lock
        let _ = tokio::spawn({
            let dir_path = dir_path.clone();
            async move {
                // one dir instance is created on a separate thread
                let dir = Dir::new(&dir_path).unwrap();
                dir.batch(|root| {
                    tx.send(()).unwrap();
                    assert_eq!(
                        std::fs::read_to_string(root.join("file.txt")).unwrap(),
                        String::new()
                    );
                    std::fs::write(root.join("file.txt"), "1")
                })
            }
        })
        .await
        .unwrap();

        // another dir instance is created on the main thread
        let dir = Dir::new(&dir_path).unwrap();

        // then we wait until the lock is aquired
        rx.recv().unwrap();

        // and immidiately try to lock again
        dir.batch(|root| {
            assert_eq!(std::fs::read_to_string(root.join("file.txt")).unwrap(), "1");
            std::fs::write(root.join("file.txt"), "2")
        })
        .unwrap();

        assert_eq!(
            std::fs::read_to_string(dir_path.join("file.txt")).unwrap(),
            "2"
        );
    }
}
