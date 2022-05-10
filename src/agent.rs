use std::{io::stdin, process::Stdio};
use tokio::{io::AsyncWriteExt, sync::watch};

use anyhow::{anyhow, Context};
use futures::{channel::mpsc, StreamExt};
use ipc_channel::{
    asynch::IpcStream,
    ipc::{self, IpcSender},
};

use crate::{
    joycon::joycon_main,
    messages::{Configuration, Status},
};

pub(crate) fn run() -> anyhow::Result<()> {
    let mut address = String::new();
    stdin()
        .read_line(&mut address)
        .context("Could not read IPC address")?;
    address.truncate(address.trim_end().len());

    let (config_tx, config_rx) =
        ipc::channel::<Configuration>().context("Could not create configuration channel")?;
    let (status_tx, status_rx) =
        ipc::channel::<Status>().context("Could not create status channel")?;

    let sender = IpcSender::connect(address).context("Could not connect to parent")?;
    sender
        .send((config_tx, status_rx))
        .context("Could not send channels")?;

    joycon_main(config_rx, status_tx).map_err(|e| anyhow!("{:?}", e))?;

    Ok(())
}

pub(crate) fn spawn() -> (mpsc::Sender<Configuration>, watch::Receiver<Status>) {
    let (config_sink, mut config_rx) = mpsc::channel(4);
    let (mut status_tx, status_receiver) = watch::channel(Status::NotConnected);

    tokio::task::spawn(async move {
        let mut last_config = None;
        loop {
            eprintln!("spawning agent");
            let (server, client) = ipc_channel::ipc::IpcOneShotServer::<(
                ipc::IpcSender<Configuration>,
                ipc::IpcReceiver<Status>,
            )>::new()
            .unwrap();
            let mut child = tokio::process::Command::new(std::env::current_exe().unwrap())
                .arg("agent")
                .stdin(Stdio::piped())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(client.as_bytes())
                .await
                .unwrap();
            let (mut config_tx, mut status_rx) = tokio::task::spawn_blocking(|| {
                let (_, (config_tx, status_rx)) = server.accept().unwrap();
                (config_tx, status_rx.to_stream())
            })
            .await
            .unwrap();

            match manage(
                &mut last_config,
                &mut config_rx,
                &mut config_tx,
                &mut status_rx,
                &mut status_tx,
                child,
            )
            .await
            {
                Ok(()) => break,
                Err(err) => eprintln!("Agent died {:?}", err),
            }
        }
    });

    (config_sink, status_receiver)
}

async fn manage(
    last_config: &mut Option<Configuration>,
    config_rx: &mut mpsc::Receiver<Configuration>,
    config_tx: &mut IpcSender<Configuration>,
    status_rx: &mut IpcStream<Status>,
    status_tx: &mut watch::Sender<Status>,
    mut child: tokio::process::Child,
) -> anyhow::Result<()> {
    if let Some(last_config) = last_config.clone() {
        config_tx.send(last_config)?;
    }

    loop {
        tokio::select! {
            (config, _) = config_rx.into_future() => {
                let config = if let Some(config) = config {
                    config
                } else {
                    return Ok(());
                };
                *last_config = Some(config.clone());
                config_tx.send(config).context("Agent send failed")?;
            }
            (status, _) = status_rx.into_future() => {
                let status = if let Some(status) = status {
                    status.context("Agent receive failed")?
                } else {
                    return Err(anyhow!("Agent connection closed"));
                };
                status_tx.send(status).context("Status forward failed")?;
            }
            _ = child.wait() => {
                return Err(anyhow!("Agent terminated"));
            }
        };
    }
}
