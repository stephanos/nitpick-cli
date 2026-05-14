use std::{io::Write, path::Path};

use atomic_write_file::AtomicWriteFile;
use fs_err as fs;

use crate::{AgentError, AgentResult};

pub fn parse_json_bytes<T: serde::de::DeserializeOwned>(
    bytes: &[u8],
    context: &str,
) -> AgentResult<T> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    serde_path_to_error::deserialize(&mut deserializer)
        .map_err(|error| AgentError::json(context, error.path(), error.inner()))
}

pub fn parse_json_str<T: serde::de::DeserializeOwned>(
    input: &str,
    context: &str,
) -> AgentResult<T> {
    let mut deserializer = serde_json::Deserializer::from_str(input);
    serde_path_to_error::deserialize(&mut deserializer)
        .map_err(|error| AgentError::json(context, error.path(), error.inner()))
}

pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> AgentResult<T> {
    let bytes = fs::read(path).map_err(|error| AgentError::io_path("read", path, error))?;
    parse_json_bytes(&bytes, &format!("parse {}", path.display()))
}

pub fn read_json_dir<T: serde::de::DeserializeOwned>(dir: &Path) -> AgentResult<Vec<T>> {
    let mut values = Vec::new();
    let mut paths = fs::read_dir(dir)
        .map_err(|error| AgentError::io_path("read directory", dir, error))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| AgentError::io_path("read directory entry in", dir, error))?;
    paths.sort();

    for path in paths {
        if path.extension().and_then(|extension| extension.to_str()) == Some("json") {
            values.push(read_json(&path)?);
        }
    }

    Ok(values)
}

pub fn write_json_atomic<T: serde::Serialize>(path: &Path, value: &T) -> AgentResult<()> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| AgentError::json(format!("serialize {}", path.display()), "$", error))?;
    let mut file =
        AtomicWriteFile::open(path).map_err(|error| AgentError::io_path("open", path, error))?;
    file.write_all(&bytes)
        .map_err(|error| AgentError::io_path("write", path, error))?;
    file.commit()
        .map_err(|error| AgentError::io_path("replace", path, error))
}
