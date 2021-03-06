use std::path::PathBuf;

use arcstr::ArcStr;
use color_eyre::eyre::Result;
use dashmap::DashMap;
use futures::future::BoxFuture;
use lateinit::LateInit;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use sled::IVec;
use tokio::sync::{mpsc::channel, oneshot};
use tracing::error;

use crate::{db::DB, OptionExt};

type Handler = dyn Fn(&(Vec<u8>, IVec)) -> BoxFuture<Result<ArcStr>> + Send + Sync + 'static;

#[derive(Singleton, Default)]
pub struct Res {
  pub directory: LateInit<PathBuf>,
  pub handlers: LateInit<DashMap<ArcStr, Vec<oneshot::Sender<PathBuf>>>>,
  pub photo_url_resolver: LateInit<Box<Handler>>,
}
impl Res {
  async fn poll(&self) -> notify::Result<()> {
    let (tx, mut rx) = channel(32);
    let mut watcher = RecommendedWatcher::new(move |res| {
      smol::block_on(async {
        tx.send(res).await.unwrap();
      });
    })?;
    watcher.watch(self.directory.as_path(), RecursiveMode::NonRecursive)?;
    while let Some(res) = rx.recv().await {
      match res {
        Ok(Event { kind, paths, .. }) => {
          if let EventKind::Create(notify::event::CreateKind::File) = kind {
            for path in paths {
              let file_name = ArcStr::from(path.file_name().unwrap().to_string_lossy());
              if self.handlers.contains_key(&file_name) {
                let (.., handler_list) = self.handlers.remove(&file_name).unwrap();
                for handler in handler_list {
                  handler.send(path.clone()).unwrap();
                }
              }
            }
          }
        }
        Err(e) => error!("watch error: {:?}", e),
      }
    }
    Ok(())
  }

  pub fn path(&self, id: &ArcStr) -> PathBuf {
    let mut path = self.directory.clone();
    path.push(id.as_str());
    path
  }

  pub fn tmp_path(&self, id: &ArcStr) -> PathBuf {
    let mut path = self.directory.clone();
    path.push(format!("{}.tmp", id));
    path
  }

  pub fn wait_for(&self, id: &ArcStr) -> oneshot::Receiver<PathBuf> {
    let (sender, receiver) = oneshot::channel();
    self
      .handlers
      .entry(id.clone())
      .or_insert(Vec::new())
      .push(sender);
    receiver
  }

  pub async fn init(&self) {
    let path = {
      let mut dir = std::env::temp_dir();
      dir.push("mesagisto");
      dir
    };
    tokio::fs::create_dir_all(path.as_path()).await.unwrap();
    self.directory.init(path);
    self.handlers.init(DashMap::default());
    tokio::spawn(async { RES.poll().await });
  }

  pub fn put_image_id<U, F>(&self, uid: U, file_id: F)
  where
    U: AsRef<[u8]>,
    F: Into<IVec>,
  {
    DB.put_image_id(uid, file_id);
  }

  pub fn resolve_photo_url<F>(&self, f: F)
  where
    F: Fn(&(Vec<u8>, IVec)) -> BoxFuture<Result<ArcStr>> + Send + Sync + 'static,
  {
    let h = Box::new(f);
    self.photo_url_resolver.init(h);
  }

  pub async fn get_photo_url<T>(&self, uid: T) -> Option<ArcStr>
  where
    T: AsRef<[u8]>,
  {
    let file_id = DB.get_image_id(&uid)?;
    let handler = &*self.photo_url_resolver;
    handler(&(uid.as_ref().to_vec(), file_id))
      .await
      .unwrap()
      .some()
  }
}

#[cfg(test)]
mod test {
  #[test]
  fn test() {
    use super::RES;
    tokio::runtime::Builder::new_multi_thread()
      .worker_threads(8)
      .enable_all()
      .build()
      .unwrap()
      .block_on(async {
        RES.init().await;
      });
  }
}
