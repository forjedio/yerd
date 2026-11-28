//! End-to-end S3 bucket administration against the **real** versitygw binary.
//!
//! Boots a daemon on a tempdir `PlatformDirs`, stages the binary into the
//! install layout, starts the gateway over IPC and drives every bucket and
//! credentials request against it, crossing one real `ListObjectsV2`
//! continuation token. Skipped unless `VERSITYGW_BIN` names a versitygw
//! executable, so CI (which has no such binary) is unaffected.
//!
//! Objects are seeded with a test-local signed `PutObject`; object upload is
//! not part of the shipped S3 request builders.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

#[cfg(unix)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use interprocess::local_socket::tokio::Stream as IpcStream;
    use interprocess::local_socket::traits::tokio::Stream as _;
    use interprocess::local_socket::{GenericFilePath, ToFsName};
    use tokio::sync::watch;
    use yerd_core::network::ResolvedProxy;
    use yerd_ipc::{
        read_message, write_message, ErrorCode, FrameDecoder, Request, Response, ServiceRunState,
        DEFAULT_MAX_FRAME,
    };
    use yerd_services::s3::sigv4;
    use yerd_services::{RootCredentials, S3_REGION};

    const VERSION: &str = "1.7.0";
    const OBJECTS: u64 = 1001;
    const ODD_KEY: &str = "x y/ü.txt";

    struct KillOnDrop(Option<u32>);

    impl Drop for KillOnDrop {
        fn drop(&mut self) {
            if let Some(pid) = self.0 {
                let _ = std::process::Command::new("kill")
                    .arg("-9")
                    .arg(pid.to_string())
                    .status();
            }
        }
    }

    fn make_dirs(tmp: &Path) -> yerd_platform::PlatformDirs {
        yerd_platform::PlatformDirs {
            config: tmp.join("c"),
            data: tmp.join("d"),
            state: tmp.join("s"),
            cache: tmp.join("ca"),
            runtime: tmp.join("r"),
        }
    }

    fn free_port() -> u16 {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    }

    fn stage_versitygw(dirs: &yerd_platform::PlatformDirs, src: &Path) {
        let bin = dirs
            .data
            .join("services")
            .join("versitygw")
            .join(VERSION)
            .join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let dst = bin.join("versitygw");
        std::fs::copy(src, &dst).unwrap();
        std::fs::set_permissions(&dst, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    async fn round_trip(sock: &Path, req: &Request) -> Response {
        let name = sock.to_fs_name::<GenericFilePath>().unwrap();
        let stream = IpcStream::connect(name).await.expect("connect");
        let (mut reader, mut writer) = stream.split();
        write_message(&mut writer, req, DEFAULT_MAX_FRAME)
            .await
            .expect("write");
        let mut decoder = FrameDecoder::new();
        read_message(&mut reader, &mut decoder)
            .await
            .expect("read")
            .expect("response")
    }

    async fn wait_running(sock: &Path) -> u32 {
        for _ in 0..80 {
            if let Response::Services { services } = round_trip(sock, &Request::ListServices).await
            {
                if let Some(s) = services.iter().find(|s| s.service == "versitygw") {
                    if let (ServiceRunState::Running, Some(pid)) = (&s.state, s.pid) {
                        return pid;
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        panic!("versitygw did not reach Running within 20s");
    }

    async fn put_object(
        client: &reqwest::Client,
        port: u16,
        creds: &RootCredentials,
        bucket: &str,
        key: &str,
        body: Vec<u8>,
    ) {
        let host = format!("127.0.0.1:{port}");
        let path = format!(
            "/{}/{}",
            sigv4::uri_encode(bucket, true),
            sigv4::uri_encode(key, false)
        );
        let payload = sigv4::sha256_hex(&body);
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let date = sigv4::AmzDate::from_unix(secs).unwrap();
        let signed = sigv4::sign(
            &sigv4::SigningInput {
                method: "PUT",
                canonical_uri: &path,
                query: &[],
                host: &host,
                payload_sha256_hex: &payload,
            },
            creds,
            S3_REGION,
            &date,
        );
        let resp = client
            .put(format!("http://{host}{path}"))
            .header("authorization", signed.authorization)
            .header("x-amz-date", signed.x_amz_date)
            .header("x-amz-content-sha256", signed.x_amz_content_sha256)
            .body(body)
            .send()
            .await
            .unwrap();
        assert!(resp.status().is_success(), "put {key}: {}", resp.status());
    }

    fn object_body(i: u64) -> Vec<u8> {
        format!("object {i}").into_bytes()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn s3_buckets_against_real_versitygw() {
        let Some(src) = std::env::var_os("VERSITYGW_BIN").map(PathBuf::from) else {
            eprintln!("skipping: set VERSITYGW_BIN to a versitygw executable to run this test");
            return;
        };
        assert!(
            src.is_file(),
            "VERSITYGW_BIN {} is not a file",
            src.display()
        );

        let tmp = tempfile::tempdir().unwrap();
        let dirs = make_dirs(tmp.path());
        stage_versitygw(&dirs, &src);

        let s3_port = free_port();
        let mut cfg = yerd_config::Config::default();
        cfg.ports.http = free_port();
        cfg.ports.https = free_port();
        cfg.dns_port = 0;
        cfg.services.instances.insert(
            "versitygw".to_owned(),
            yerd_config::ServiceInstance {
                version: Some(VERSION.to_owned()),
                port: Some(s3_port),
                ..yerd_config::ServiceInstance::default()
            },
        );
        let cfg_path = dirs.config.join("yerd.toml");

        let daemon = yerdd::startup::bring_up_with_dirs(dirs.clone(), cfg, cfg_path)
            .await
            .expect("bring_up_with_dirs");
        let sock = dirs.runtime.join("yerd.sock");
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let ipc_task = tokio::spawn(yerdd::ipc_server::run(
            daemon.ipc_listener,
            daemon.state.clone(),
            shutdown_rx,
        ));
        tokio::time::sleep(Duration::from_millis(100)).await;

        let started = round_trip(
            &sock,
            &Request::StartService {
                service: "versitygw".to_owned(),
            },
        )
        .await;
        assert!(
            !matches!(started, Response::Error { .. }),
            "start failed: {started:?}"
        );
        let mut guard = KillOnDrop(Some(wait_running(&sock).await));

        exercise(&sock, s3_port).await;

        let stopped = round_trip(
            &sock,
            &Request::StopService {
                service: "versitygw".to_owned(),
            },
        )
        .await;
        assert!(matches!(stopped, Response::Ok), "stop: {stopped:?}");
        guard.0 = None;
        match round_trip(&sock, &Request::ListBuckets).await {
            Response::Error { code, message } => {
                assert_eq!(code, ErrorCode::Internal);
                assert!(
                    message.starts_with("start S3 (versitygw) first"),
                    "{message}"
                );
            }
            other => panic!("expected error after stop, got {other:?}"),
        }

        shutdown_tx.send_replace(true);
        let _ = tokio::time::timeout(Duration::from_secs(10), ipc_task).await;
    }

    #[allow(clippy::too_many_lines)]
    async fn exercise(sock: &Path, s3_port: u16) {
        let (endpoint, access_key) =
            match round_trip(sock, &Request::S3Credentials { show_secret: false }).await {
                Response::S3Credentials {
                    endpoint,
                    access_key,
                    secret_key,
                } => {
                    assert!(secret_key.is_none(), "secret must be hidden");
                    (endpoint, access_key)
                }
                other => panic!("expected s3_credentials, got {other:?}"),
            };
        assert_eq!(endpoint, format!("http://127.0.0.1:{s3_port}"));
        let secret_key = match round_trip(sock, &Request::S3Credentials { show_secret: true }).await
        {
            Response::S3Credentials {
                secret_key: Some(secret),
                ..
            } => secret,
            other => panic!("expected a revealed secret, got {other:?}"),
        };
        let creds = RootCredentials {
            access_key,
            secret_key,
        };

        for name in ["media", "health"] {
            let created = round_trip(
                sock,
                &Request::CreateBucket {
                    name: name.to_owned(),
                },
            )
            .await;
            assert!(
                matches!(created, Response::Ok),
                "create {name}: {created:?}"
            );
        }
        match round_trip(
            sock,
            &Request::CreateBucket {
                name: "media".to_owned(),
            },
        )
        .await
        {
            Response::Error { code, .. } => assert_eq!(code, ErrorCode::AlreadyExists),
            other => panic!("expected already_exists, got {other:?}"),
        }

        let client = yerdd::network::build_client(&ResolvedProxy::off()).unwrap();
        let mut total_bytes: u64 = 0;
        for i in 0..OBJECTS {
            let key = if i == 0 {
                ODD_KEY.to_owned()
            } else {
                format!("obj/{i:05}")
            };
            let body = object_body(i);
            total_bytes += body.len() as u64;
            put_object(&client, s3_port, &creds, "media", &key, body).await;
        }

        let stats = round_trip(
            sock,
            &Request::BucketStats {
                name: "media".to_owned(),
            },
        )
        .await;
        assert_eq!(
            stats,
            Response::BucketStats {
                objects: OBJECTS,
                bytes: total_bytes,
            }
        );

        let dropped = round_trip(
            sock,
            &Request::DropBucket {
                name: "media".to_owned(),
            },
        )
        .await;
        assert_eq!(
            dropped,
            Response::BucketDropped {
                objects_removed: OBJECTS
            }
        );
        match round_trip(sock, &Request::ListBuckets).await {
            Response::Buckets { buckets } => {
                let names: Vec<&str> = buckets.iter().map(|b| b.name.as_str()).collect();
                assert_eq!(names, vec!["health"]);
            }
            other => panic!("expected buckets, got {other:?}"),
        }

        let health = round_trip(
            sock,
            &Request::BucketStats {
                name: "health".to_owned(),
            },
        )
        .await;
        assert_eq!(
            health,
            Response::BucketStats {
                objects: 0,
                bytes: 0
            }
        );
        let dropped = round_trip(
            sock,
            &Request::DropBucket {
                name: "health".to_owned(),
            },
        )
        .await;
        assert_eq!(dropped, Response::BucketDropped { objects_removed: 0 });
        match round_trip(
            sock,
            &Request::BucketStats {
                name: "health".to_owned(),
            },
        )
        .await
        {
            Response::Error { code, .. } => assert_eq!(code, ErrorCode::NotFound),
            other => panic!("expected not_found, got {other:?}"),
        }
    }
}
