use crate::{HostClientError, HostClientResult};

#[derive(serde::Serialize)]
pub(crate) struct ArtifactSyncInput<'a> {
    pub(crate) destination: &'a str,
    pub(crate) target: Option<&'a str>,
}

#[derive(serde::Serialize)]
pub(crate) struct ResetLocalStateInput {
    pub(crate) force: bool,
}

pub(crate) fn request_host(
    agent: &ureq::Agent,
    addr: &str,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
) -> HostClientResult<String> {
    let url = format!("http://{addr}{path}");
    let result = match (method, body) {
        ("GET", None) => agent.get(&url).call(),
        ("POST", Some(body)) => agent
            .post(&url)
            .header("Content-Type", "application/json")
            .send(body),
        ("POST", None) => agent.post(&url).send(&[]),
        ("GET", Some(_)) => return Err(HostClientError::GetWithBody),
        _ => {
            return Err(HostClientError::UnsupportedMethod {
                method: method.to_owned(),
            });
        }
    };
    let mut response = match result {
        Ok(response) => response,
        Err(error) => {
            return Err(HostClientError::Unavailable {
                addr: addr.to_owned(),
                message: error.to_string(),
            });
        }
    };
    response
        .body_mut()
        .read_to_string()
        .map_err(|error| HostClientError::ReadResponse {
            addr: addr.to_owned(),
            path: path.to_owned(),
            message: error.to_string(),
        })
        .and_then(|body| {
            let status = response.status();
            if status.is_success() {
                Ok(body)
            } else {
                let details = body.trim();
                if details.is_empty() {
                    Err(HostClientError::HttpStatus {
                        status: status.to_string(),
                    })
                } else {
                    Err(HostClientError::HttpStatusWithBody {
                        status: status.to_string(),
                        body: details.to_owned(),
                    })
                }
            }
        })
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    use crate::HostClientError;

    use super::request_host;

    #[test]
    fn request_host_includes_error_response_body() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request = [0_u8; 512];
            let _ = stream.read(&mut request).expect("read request");
            stream
                .write_all(
                    b"HTTP/1.1 400 Bad Request\r\nContent-Length: 19\r\nConnection: close\r\n\r\nbad request details",
                )
                .expect("write response");
        });

        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build();
        let agent = ureq::Agent::new_with_config(config);
        let error = request_host(&agent, &addr.to_string(), "GET", "/status", None)
            .expect_err("status error");
        handle.join().expect("server thread");

        assert!(matches!(
            error,
            HostClientError::HttpStatusWithBody { body, .. } if body == "bad request details"
        ));
    }
}
