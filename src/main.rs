use std::{
    io::{BufRead, BufReader},
    process::{Child, ChildStdout, Command, Stdio},
};

use audio::{get_audio_data, YOUTUBE_TS_SAMPLE_RATE};
use clap::{arg, command, Parser};
use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    SampleFormat, SampleRate,
};
use ringbuf::{HeapRb, LocalRb};

mod audio;

#[derive(Parser, Debug, Clone)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// youtube url or youtube video id
    #[arg()]
    url: String,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
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

    // get youtube streaming
    loop {
        let buf = reader.fill_buf().unwrap();
        if buf.is_empty() {
            break;
        }

        let len = buf.len();
        ts_prod.push_slice(buf);

        if ts_prod.is_full() || ts_prod.len() >= 64 * 1024 {
            println!("parsing {:.2}kb ts file", ts_prod.len() as f32 / 1024.0);
            let audio_data = ts_cons.pop_iter().collect::<Vec<u8>>();

            if let Ok(audio_data) = get_audio_data(&audio_data) {
                println!("get {:.2}kb audio data", audio_data.len() as f32 / 1024.0);

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
    Ok(())
}

fn get_yt_dlp_stdout(url: &str) -> (Child, ChildStdout) {
    let mut cmd = Command::new("yt-dlp");
    cmd.arg(url)
        .args(["-f", "w"])
        .args(["--quiet"])
        .args(["-o", "-"]);

    let mut child = cmd
        .stdout(Stdio::piped())
        .spawn()
        .expect("failed to execute yt-dlp");

    let stdout = child.stdout.take().expect("invalid stdout stream");

    (child, stdout)
}
