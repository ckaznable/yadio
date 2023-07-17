use std::{
    io::{BufRead, BufReader},
    mem::MaybeUninit,
    process::{Child, ChildStdout, Command, Stdio},
    sync::Arc,
    time::Duration,
};

use audio::{get_audio_data, YOUTUBE_TS_SAMPLE_RATE};
use clap::{arg, command, Parser};
use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    Device, SampleFormat, SampleRate, Stream, SupportedStreamConfig,
};
use owo_colors::OwoColorize;
use ringbuf::{Consumer, HeapRb, LocalRb, SharedRb};
use tokio::{
    task::{self, JoinHandle},
    time,
};
use youtube_chat::{
    item::{ChatItem, MessageItem},
    live_chat::LiveChatClientBuilder,
};

mod audio;

#[derive(Parser, Debug, Clone)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// youtube url or youtube video id
    #[arg()]
    url: String,

    /// enable chatroom output
    #[arg(long, default_value_t = false)]
    chatroom: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let is_enable_chatroom = args.chatroom;
    let (mut child, stdout) = get_yt_dlp_stdout(args.url.as_ref());
    let mut reader = BufReader::new(stdout);

    // 15sec audio sample buffer
    let rb_len = YOUTUBE_TS_SAMPLE_RATE as usize * 2 * 20;
    let rb = HeapRb::<f32>::new(rb_len);
    let (mut prod, cons) = rb.split();

    // 256kb ts buffer
    let rb_len = 256 * 1024;
    let rb = LocalRb::<u8, Vec<_>>::new(rb_len);
    let (mut ts_prod, mut ts_cons) = rb.split();

    let (device, config) = get_output_device_and_config();
    let stream = output_stream(device, config, cons).unwrap();

    // 3sec startup buffer
    let mut startup_buffer_flag = true;
    let startup_target_size = YOUTUBE_TS_SAMPLE_RATE as usize * 2 * 10;
    let mut startup_buffer: Vec<f32> = vec![];

    let chat_stream_handle = if is_enable_chatroom {
        Some(chat_streaming(args.url.as_ref()).await)
    } else {
        None
    };

    // get youtube streaming
    loop {
        if prod.is_empty() && startup_buffer.is_empty() {
            startup_buffer_flag = true;
        }

        let buf = reader.fill_buf().unwrap();
        if buf.is_empty() {
            break;
        }

        let len = buf.len();
        ts_prod.push_slice(buf);

        if ts_prod.is_full() || ts_prod.len() >= 32 * 1024 {
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

    stream.pause()?;
    child.kill().expect("failed to kill yt-dlp process");
    if let Some(handle) = chat_stream_handle {
        handle.await?;
    }
    Ok(())
}

fn get_output_device_and_config() -> (Device, SupportedStreamConfig) {
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

    (device, config)
}

fn get_yt_dlp_stdout(url: &str) -> (Child, ChildStdout) {
    let mut cmd = Command::new("yt-dlp");
    cmd.arg(url)
        .args(["-f", "w"])
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

type RbConsumer = Consumer<f32, Arc<SharedRb<f32, Vec<MaybeUninit<f32>>>>>;
fn output_stream(
    device: Device,
    config: SupportedStreamConfig,
    mut cons: RbConsumer,
) -> Result<Stream, anyhow::Error> {
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

    Ok(stream)
}

async fn chat_streaming(url: &str) -> JoinHandle<()> {
    let builder = if url.starts_with("https") {
        LiveChatClientBuilder::new().url(url).unwrap()
    } else {
        LiveChatClientBuilder::new().live_id(url.to_string())
    };

    let mut client = builder
        .on_chat(|chat_item: ChatItem| {
            if let Some(name) = chat_item.author.name {
                chat_item.message.into_iter().for_each(|message| {
                    if let MessageItem::Text(text) = message {
                        println!("{}: {}", name.yellow(), text);
                    }
                })
            };
        })
        .on_error(|_| {})
        .build();

    client.start().await.unwrap();
    task::spawn(async move {
        let mut interval = time::interval(Duration::from_millis(5000));
        loop {
            interval.tick().await;
            client.execute().await;
        }
    })
}
