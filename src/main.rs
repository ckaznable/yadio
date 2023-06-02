use std::{
    io::{BufRead, BufReader},
    process::{Child, ChildStdout, Command, Stdio}, time::Duration,
};

use audio::{get_audio_data, YOUTUBE_TS_SAMPLE_RATE};
use clap::{arg, command, Parser};
use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    SampleFormat, SampleRate,
};
use ringbuf::{HeapRb, LocalRb};
use tokio::{task::{self, JoinHandle}, time};
use youtube_chat::{live_chat::LiveChatClientBuilder, item::{ChatItem, MessageItem}};

mod audio;

#[derive(Parser, Debug, Clone)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// youtube url or youtube video id
    #[arg()]
    url: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let (mut child, stdout) = get_yt_dlp_stdout(args.url.as_ref());
    let mut reader = BufReader::new(stdout);

    let host = cpal::default_host();
    let device = host.default_output_device().unwrap();
    let mut supported_configs_range = device
        .supported_output_configs()
        .expect("error while querying configs");

    let target_sample_rate = SampleRate(YOUTUBE_TS_SAMPLE_RATE);
    let config = supported_configs_range
        .find(|config| {
            config.max_sample_rate() > target_sample_rate
                && config.min_sample_rate() < target_sample_rate
                && config.sample_format() == SampleFormat::F32
                && config.channels() == 2
        })
        .expect("no supported config found")
        .with_sample_rate(target_sample_rate);

    // 15sec audio sample buffer
    let rb_len = YOUTUBE_TS_SAMPLE_RATE as usize * 2 * 20;
    let rb = HeapRb::<f32>::new(rb_len);
    let (mut prod, mut cons) = rb.split();

    // 256kb ts buffer
    let rb_len = 256 * 1024;
    let rb = LocalRb::<u8, Vec<_>>::new(rb_len);
    let (mut ts_prod, mut ts_cons) = rb.split();

    let err_fn = |err| eprintln!("an error occurred on stream: {}", err);
    let stream = device.build_output_stream(
        &config.into(),
        move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
            let written = cons.pop_slice(data);
            data[written..].iter_mut().for_each(|s| *s = 0.0);
        },
        err_fn,
        None,
    )?;
    stream.play()?;

    // 3sec startup buffer
    let mut startup_buffer_flag = true;
    let startup_target_size = YOUTUBE_TS_SAMPLE_RATE as usize * 2 * 5;
    let mut startup_buffer: Vec<f32> = vec![];

    let chat_stream_handle = chat_streaming(args.url.as_ref()).await;

    // get youtube streaming
    loop {
        let buf = reader.fill_buf().unwrap();
        if buf.is_empty() {
            break;
        }

        let len = buf.len();
        ts_prod.push_slice(buf);

        if ts_prod.is_full() || ts_prod.len() >= 64 * 1024 {
            let audio_data = ts_cons.pop_iter().collect::<Vec<u8>>();

            if let Ok(audio_data) = get_audio_data(&audio_data) {
                if !startup_buffer_flag {
                    prod.push_slice(&audio_data);
                } else {
                    startup_buffer.extend(audio_data.into_iter());

                    if startup_buffer.len() >= startup_target_size {
                        startup_buffer_flag = false;
                        prod.push_slice(&startup_buffer);
                        startup_buffer.clear();
                    }
                }
            }
        }

        reader.consume(len);
    }

    child.kill().expect("failed to kill yt-dlp process");
    let _ = chat_stream_handle.await;
    Ok(())
}

fn get_yt_dlp_stdout(url: &str) -> (Child, ChildStdout) {
    let mut cmd = Command::new("yt-dlp");
    cmd.arg(url)
        .args(["-S", "+size,+br,res"])
        .args(["--compat-options", "no-direct-merge"])
        .args(["--quiet"])
        .args(["-o", "-"]);

    let mut child = cmd
        .stdout(Stdio::piped())
        .spawn()
        .expect("failed to execute yt-dlp");

    let stdout = child.stdout.take().expect("invalid stdout stream");

    (child, stdout)
}

async fn chat_streaming(url: &str) -> JoinHandle<()> {
    let on_chat = |chat_item: ChatItem| {
        if let Some(name) = chat_item.author.name {
            chat_item.message.into_iter().for_each(|message| {
                if let MessageItem::Text(text) = message {
                    println!("{}: {}", name, text);
                }
            })
        };
    };

    let on_error = |e: anyhow::Error| eprintln!("error: {}", e);

    let mut client = if url.starts_with("https") {
        LiveChatClientBuilder::new()
            .url(url)
            .unwrap()
            .on_chat(on_chat)
            .on_error(on_error)
            .build()
    } else {
        LiveChatClientBuilder::new()
            .live_id(url.to_string())
            .on_chat(on_chat)
            .on_error(on_error)
            .build()
    };

    client.start().await.unwrap();
    task::spawn(async move {
        let mut interval = time::interval(Duration::from_millis(3000));
        loop {
            interval.tick().await;
            client.execute().await;
        }
    })
}