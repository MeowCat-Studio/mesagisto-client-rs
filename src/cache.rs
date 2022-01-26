use std::panic;
use std::path::PathBuf;
use std::time::Duration;

use crate::data::events::{Event, EventType};
use crate::data::Packet;
use crate::net::NET;
use crate::res::RES;
use crate::server::SERVER;
use crate::EitherExt;
use arcstr::ArcStr;
use thiserror::Error;
use tracing::trace;

#[derive(Error, Debug)]
pub enum CacheError {
  #[error(transparent)]
  RecvError(#[from] tokio::sync::oneshot::error::RecvError),
  #[error(transparent)]
  DataError(#[from] crate::data::DataError),
  #[error(transparent)]
  ServerError(#[from] crate::server::ServerError),
  #[error(transparent)]
  IoError(#[from] std::io::Error),
  #[error("Timeout error when requesting image url")]
  TimeoutError(#[from] tokio::time::error::Elapsed),
  #[error(transparent)]
  HttpError(#[from] reqwest::Error),
  #[error(transparent)]
  AnyhowError(#[from] anyhow::Error),
}

#[derive(Singleton, Default)]
pub struct Cache {}

impl Cache {
  pub fn init(&self) {}

  pub async fn file(
    &self,
    id: &Vec<u8>,
    url: &Option<ArcStr>,
    address: &ArcStr,
  ) -> Result<PathBuf, CacheError> {
    match url {
      Some(url) => self.file_by_url(id, url).await,
      None => self.file_by_uid(id, address).await,
    }
  }

  pub async fn file_by_uid(&self, uid: &Vec<u8>, address: &ArcStr) -> Result<PathBuf, CacheError> {
    let uid_str: ArcStr = base64_url::encode(uid).into();
    trace!("Caching file by uid {}", uid_str);
    let path = RES.path(&uid_str);
    if path.exists() {
      trace!("File exists,return the path");
      return Ok(path);
    }
    let tmp_path = RES.tmp_path(&uid_str);
    if tmp_path.exists() {
      trace!("TmpFile exists,waiting for the file downloading");
      return Ok(RES.wait_for(&uid_str).await?);
    }
    trace!("TmpFile dont exist,requesting image url");
    let packet: Event = EventType::RequestImage { id: uid.clone() }.into();
    // fixme error handling
    let packet = Packet::from(packet.to_right())?;
    // fixme timeout check
    let response = SERVER.request(address, packet, Some(&*SERVER.lib_header));
    let response = tokio::time::timeout(Duration::from_secs(5), response).await??;
    trace!("Get the image respond");
    let r_packet = Packet::from_cbor(&response.data)?;
    match r_packet {
      either::Either::Right(event) => match event.data {
        EventType::RespondImage { id, url } => self.file_by_url(&id, &url).await,
        _ => panic!("Not correct response"),
      },
      either::Either::Left(_) => panic!("Not correct response"),
    }
  }
  pub async fn file_by_url(&self, id: &Vec<u8>, url: &ArcStr) -> Result<PathBuf, CacheError> {
    let id_str: ArcStr = base64_url::encode(id).into();
    let path = RES.path(&id_str);
    if path.exists() {
      return Ok(path);
    }

    let tmp_path = RES.tmp_path(&id_str);
    return if tmp_path.exists() {
      let fut = RES.wait_for(&id_str);
      let path = tokio::time::timeout(std::time::Duration::from_secs(5), fut).await??;
      Ok(path)
    } else {
      // fixme error handling
      NET.download(url, &tmp_path).await?;
      tokio::fs::rename(&tmp_path, &path).await?;
      Ok(path)
    };
  }

  pub async fn put_file(&self, id: &Vec<u8>, file: &PathBuf) -> Result<PathBuf, CacheError> {
    let id_str: ArcStr = base64_url::encode(id).into();
    let path = RES.path(&id_str);
    tokio::fs::rename(&file, &path).await?;
    Ok(path)
  }
}
