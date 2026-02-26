use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use serde_json::{json, Value};

use pterminal_core::config::theme::RgbColor;
use pterminal_core::config::Theme;
use pterminal_core::terminal::{GridLine, TerminalEmulator};
use pterminal_core::PaneId;
use pterminal_ipc::IpcClient;
use pterminal_render::text::{PixelRect, TextRenderer};
use pterminal_render::BgRenderer;

#[derive(Debug, Parser)]
#[command(name = "pterminal-cli", about = "Control pterminal via JSON-RPC IPC")]
struct Cli {
    /// Override socket path (default: ~/.config/pterminal/pterminal.sock)
    #[arg(long)]
    socket: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Ping,
    Capabilities,
    Identify,
    ListWorkspaces,
    NewWorkspace,
    CloseWorkspace {
        #[arg(long)]
        id: Option<u64>,
    },
    SelectWorkspace {
        #[arg(long)]
        id: Option<u64>,
        #[arg(long)]
        index: Option<usize>,
    },
    ListPanes,
    Send {
        text: String,
        #[arg(long)]
        pane_id: Option<u64>,
    },
    ReadScreen {
        #[arg(long)]
        pane_id: Option<u64>,
    },
    CapturePane {
        #[arg(long)]
        pane_id: Option<u64>,
    },
    Notify {
        title: String,
        body: Option<String>,
    },
    ListNotifications,
    ClearNotifications,
    Bench {
        #[arg(long, default_value_t = 120)]
        cols: u16,
        #[arg(long, default_value_t = 40)]
        rows: u16,
        #[arg(long, default_value_t = 200)]
        iterations: usize,
    },
    Rpc {
        method: String,
        #[arg(long, default_value = "{}")]
        params: String,
    },
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    if let Command::Bench {
        cols,
        rows,
        iterations,
    } = &cli.command
    {
        run_bench(*cols, *rows, *iterations).await?;
        return Ok(());
    }

    let socket = cli.socket.unwrap_or_else(IpcClient::default_socket_path);
    let client = IpcClient::new(socket);

    let result = match cli.command {
        Command::Ping => client.call("ping", json!({})).await?,
        Command::Capabilities => client.call("capabilities", json!({})).await?,
        Command::Identify => client.call("identify", json!({})).await?,
        Command::ListWorkspaces => client.call("workspace.list", json!({})).await?,
        Command::NewWorkspace => client.call("workspace.new", json!({})).await?,
        Command::CloseWorkspace { id } => {
            client.call("workspace.close", json!({ "id": id })).await?
        }
        Command::SelectWorkspace { id, index } => {
            if id.is_none() && index.is_none() {
                return Err(anyhow!("either --id or --index is required"));
            }
            client
                .call("workspace.select", json!({ "id": id, "index": index }))
                .await?
        }
        Command::ListPanes => client.call("pane.list", json!({})).await?,
        Command::Send { text, pane_id } => {
            client
                .call("terminal.send", json!({ "text": text, "pane_id": pane_id }))
                .await?
        }
        Command::ReadScreen { pane_id } => {
            client
                .call("pane.read_screen", json!({ "pane_id": pane_id }))
                .await?
        }
        Command::CapturePane { pane_id } => {
            client
                .call("pane.capture", json!({ "pane_id": pane_id }))
                .await?
        }
        Command::Notify { title, body } => {
            client
                .call(
                    "notification.send",
                    json!({
                        "title": title,
                        "body": body.unwrap_or_default()
                    }),
                )
                .await?
        }
        Command::ListNotifications => client.call("notification.list", json!({})).await?,
        Command::ClearNotifications => client.call("notification.clear", json!({})).await?,
        Command::Bench { .. } => unreachable!("handled before IPC client init"),
        Command::Rpc { method, params } => {
            let value: Value = serde_json::from_str(&params)
                .with_context(|| format!("failed to parse --params JSON: {params}"))?;
            client.call(&method, value).await?
        }
    };

    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

async fn run_bench(cols: u16, rows: u16, iterations: usize) -> Result<()> {
    let theme = Arc::new(Theme::default());

    let throughput = bench_throughput_ls_like(&theme, cols, rows, iterations);
    let scrollback = bench_scrollback(&theme, cols, rows, iterations);
    let clear_screen = bench_clear_screen_ctrl_l(&theme, cols, rows, iterations);
    let selection_drag = bench_selection_drag(&theme, cols, rows, iterations);
    let split_scene = bench_split_scene(&theme, cols, rows, iterations);
    let render_breakdown = match bench_render_pipeline(&theme, cols, rows, iterations).await {
        Ok(v) => v,
        Err(e) => json!({
            "name": "render_pipeline_breakdown",
            "error": e.to_string(),
        }),
    };

    let report = json!({
        "benchmarks": [throughput, scrollback, clear_screen, selection_drag, split_scene, render_breakdown],
        "params": {
            "cols": cols,
            "rows": rows,
            "iterations": iterations
        }
    });
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn bench_throughput_ls_like(theme: &Arc<Theme>, cols: u16, rows: u16, iterations: usize) -> Value {
    let emu = TerminalEmulator::new(cols, rows);
    let mut snapshot = Vec::new();
    let mut total_bytes = 0usize;
    let mut total_dirty_rows = 0usize;

    let start = Instant::now();
    for i in 0..iterations {
        let payload = generate_ls_like_burst(i, 64);
        total_bytes += payload.len();
        emu.process(&payload);
        let delta = emu.extract_grid_delta_into(theme, &mut snapshot);
        total_dirty_rows += if delta.full {
            snapshot.len()
        } else {
            delta.dirty_rows.len()
        };
    }
    metric_json(
        "throughput_ls_like",
        iterations,
        start.elapsed().as_secs_f64(),
        total_bytes,
        total_dirty_rows,
    )
}

fn bench_scrollback(theme: &Arc<Theme>, cols: u16, rows: u16, iterations: usize) -> Value {
    let emu = TerminalEmulator::new(cols, rows);
    let mut snapshot = Vec::new();
    let mut total_bytes = 0usize;
    let mut total_dirty_rows = 0usize;

    let start = Instant::now();
    for i in 0..iterations {
        let payload = generate_line_flood(i * 256, 256);
        total_bytes += payload.len();
        emu.process(&payload);
        let delta = emu.extract_grid_delta_into(theme, &mut snapshot);
        total_dirty_rows += if delta.full {
            snapshot.len()
        } else {
            delta.dirty_rows.len()
        };
    }
    metric_json(
        "scrollback_flood",
        iterations,
        start.elapsed().as_secs_f64(),
        total_bytes,
        total_dirty_rows,
    )
}

fn bench_clear_screen_ctrl_l(theme: &Arc<Theme>, cols: u16, rows: u16, iterations: usize) -> Value {
    let emu = TerminalEmulator::new(cols, rows);
    let mut snapshot = Vec::new();
    // Prime with enough content so clear-screen does real work.
    emu.process(&generate_line_flood(0, rows as usize * 4));
    let _ = emu.extract_grid_delta_into(theme, &mut snapshot);

    let mut total_bytes = 0usize;
    let mut total_dirty_rows = 0usize;
    let start = Instant::now();
    for i in 0..iterations {
        let payload = generate_ctrl_l_clear_payload(i);
        total_bytes += payload.len();
        emu.process(&payload);
        let delta = emu.extract_grid_delta_into(theme, &mut snapshot);
        total_dirty_rows += if delta.full {
            snapshot.len()
        } else {
            delta.dirty_rows.len()
        };
    }

    metric_json(
        "clear_screen_ctrl_l_like",
        iterations,
        start.elapsed().as_secs_f64(),
        total_bytes,
        total_dirty_rows,
    )
}

fn bench_selection_drag(theme: &Arc<Theme>, cols: u16, rows: u16, iterations: usize) -> Value {
    let emu = TerminalEmulator::new(cols, rows);
    let mut snapshot = Vec::new();
    // Prime with a full screen of content.
    emu.process(&generate_line_flood(0, rows as usize * 2));
    let _ = emu.extract_grid_delta_into(theme, &mut snapshot);

    let mut visited_cells = 0usize;
    let mut checksum = 0u64;
    let max_row = snapshot.len().saturating_sub(1) as u16;
    let max_col = snapshot
        .first()
        .map(|l| l.cells.len().saturating_sub(1) as u16)
        .unwrap_or(0);

    let start = Instant::now();
    for i in 0..iterations.max(1) * 20 {
        let r0 = (i as u16) % (max_row.saturating_add(1).max(1));
        let c0 = ((i * 7) as u16) % (max_col.saturating_add(1).max(1));
        let r1 = ((i * 3 + 11) as u16) % (max_row.saturating_add(1).max(1));
        let c1 = ((i * 13 + 5) as u16) % (max_col.saturating_add(1).max(1));
        let ((sc, sr), (ec, er)) = normalize_sel((c0, r0), (c1, r1));
        let (cells, sum) = scan_selection_region(&snapshot, (sc, sr), (ec, er));
        visited_cells += cells;
        checksum = checksum.wrapping_add(sum);
    }
    let elapsed = start.elapsed().as_secs_f64();

    json!({
        "name": "selection_drag_cpu",
        "iterations": iterations.max(1) * 20,
        "total_ms": elapsed * 1000.0,
        "avg_ms": (elapsed * 1000.0) / (iterations.max(1) * 20) as f64,
        "visited_cells": visited_cells,
        "checksum": checksum,
    })
}

fn bench_split_scene(theme: &Arc<Theme>, cols: u16, rows: u16, iterations: usize) -> Value {
    let pane_count = 4usize;
    let mut panes: Vec<(TerminalEmulator, Vec<GridLine>)> = (0..pane_count)
        .map(|_| (TerminalEmulator::new(cols / 2, rows / 2), Vec::new()))
        .collect();

    let mut total_bytes = 0usize;
    let mut total_dirty_rows = 0usize;
    let start = Instant::now();

    for frame in 0..iterations {
        for (pane_idx, (emu, snapshot)) in panes.iter_mut().enumerate() {
            // Update only some panes each frame to simulate split panes with uneven activity.
            if (frame + pane_idx) % 2 == 0 {
                let payload = generate_ls_like_burst(frame * 17 + pane_idx, 24);
                total_bytes += payload.len();
                emu.process(&payload);
            }
            let delta = emu.extract_grid_delta_into(theme, snapshot);
            total_dirty_rows += if delta.full {
                snapshot.len()
            } else {
                delta.dirty_rows.len()
            };
        }
    }

    metric_json(
        "split_scene_4pane",
        iterations * pane_count,
        start.elapsed().as_secs_f64(),
        total_bytes,
        total_dirty_rows,
    )
}

async fn bench_render_pipeline(
    theme: &Arc<Theme>,
    cols: u16,
    rows: u16,
    iterations: usize,
) -> Result<Value> {
    let iterations = iterations.max(1);
    let pane_id: PaneId = 1;

    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::all(),
        ..Default::default()
    });
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        })
        .await?;

    let (device, queue) = adapter
        .request_device(
            &wgpu::DeviceDescriptor {
                label: Some("pterminal-cli-bench"),
                ..Default::default()
            },
        )
        .await?;

    let format = wgpu::TextureFormat::Bgra8Unorm;
    let width = ((cols as f32 * 9.6) as u32 + 24).max(640);
    let height = ((rows as f32 * 18.5) as u32 + 24).max(360);
    let mut text_renderer = TextRenderer::new(&device, &queue, format, width, height, 1.0, 14.0);
    let mut bg_renderer = BgRenderer::new(&device, &queue, format, width, height);

    let pane_rect = PixelRect {
        x: 8.0,
        y: 8.0,
        w: (width as f32 - 16.0).max(1.0),
        h: (height as f32 - 16.0).max(1.0),
    };
    let pane_rects = vec![(pane_id, pane_rect)];

    let offscreen = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("bench_offscreen"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let offscreen_view = offscreen.create_view(&wgpu::TextureViewDescriptor::default());

    let emu = TerminalEmulator::new(cols, rows);
    let mut snapshot = Vec::new();
    emu.process(&generate_line_flood(0, rows as usize * 2));
    let _ = emu.extract_grid_delta_into(theme, &mut snapshot);

    let mut stage_grid_ms = 0.0f64;
    let mut stage_text_update_ms = 0.0f64;
    let mut stage_text_prepare_ms = 0.0f64;
    let mut stage_bg_prepare_ms = 0.0f64;
    let mut stage_render_ms = 0.0f64;
    let mut total_bytes = 0usize;
    let mut total_dirty_rows = 0usize;
    let mut total_bg_rects = 0usize;

    for i in 0..iterations {
        let payload = generate_ls_like_burst(i, 64);
        total_bytes += payload.len();

        let t_grid = Instant::now();
        emu.process(&payload);
        let delta = emu.extract_grid_delta_into(theme, &mut snapshot);
        let cursor_pos = emu.cursor_position();
        stage_grid_ms += t_grid.elapsed().as_secs_f64() * 1000.0;

        let dirty_rows_storage;
        let dirty_rows: &[usize] = if delta.full {
            dirty_rows_storage = (0..snapshot.len()).collect::<Vec<_>>();
            &dirty_rows_storage
        } else {
            &delta.dirty_rows
        };
        total_dirty_rows += dirty_rows.len();

        let t_text_update = Instant::now();
        text_renderer.set_pane_content(
            pane_id,
            &snapshot,
            Some(dirty_rows),
            cursor_pos,
            true,
            theme.colors.cursor,
            theme.colors.background,
            None,
            theme.colors.selection_bg,
        );
        stage_text_update_ms += t_text_update.elapsed().as_secs_f64() * 1000.0;

        let t_bg_prepare = Instant::now();
        let bg_rects = text_renderer.collect_bg_rects(&pane_rects);
        total_bg_rects += bg_rects.len();
        bg_renderer.prepare(&device, &queue, &bg_rects, width, height);
        stage_bg_prepare_ms += t_bg_prepare.elapsed().as_secs_f64() * 1000.0;

        let t_text_prepare = Instant::now();
        text_renderer.prepare_panes(&device, &queue, &pane_rects, theme.colors.foreground);
        stage_text_prepare_ms += t_text_prepare.elapsed().as_secs_f64() * 1000.0;

        let t_render = Instant::now();
        let bg = color_to_wgpu(theme.colors.background);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("bench_render_encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("bench_render_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &offscreen_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(bg),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            bg_renderer.render(&mut pass);
            text_renderer.render(&mut pass);
        }
        queue.submit(std::iter::once(encoder.finish()));
        text_renderer.post_render();
        stage_render_ms += t_render.elapsed().as_secs_f64() * 1000.0;
    }

    Ok(json!({
        "name": "render_pipeline_breakdown",
        "iterations": iterations,
        "bytes": total_bytes,
        "dirty_rows": total_dirty_rows,
        "bg_rects": total_bg_rects,
        "avg_ms": (stage_grid_ms + stage_text_update_ms + stage_bg_prepare_ms + stage_text_prepare_ms + stage_render_ms) / iterations as f64,
        "stages_ms": {
            "grid_apply_delta": stage_grid_ms,
            "text_update_buffers": stage_text_update_ms,
            "bg_prepare": stage_bg_prepare_ms,
            "text_prepare": stage_text_prepare_ms,
            "render_encode_submit": stage_render_ms,
        },
        "stages_avg_ms": {
            "grid_apply_delta": stage_grid_ms / iterations as f64,
            "text_update_buffers": stage_text_update_ms / iterations as f64,
            "bg_prepare": stage_bg_prepare_ms / iterations as f64,
            "text_prepare": stage_text_prepare_ms / iterations as f64,
            "render_encode_submit": stage_render_ms / iterations as f64,
        }
    }))
}

fn color_to_wgpu(color: RgbColor) -> wgpu::Color {
    wgpu::Color {
        r: color.r as f64 / 255.0,
        g: color.g as f64 / 255.0,
        b: color.b as f64 / 255.0,
        a: 1.0,
    }
}

fn metric_json(
    name: &str,
    iterations: usize,
    elapsed_secs: f64,
    bytes: usize,
    dirty_rows: usize,
) -> Value {
    let total_ms = elapsed_secs * 1000.0;
    json!({
        "name": name,
        "iterations": iterations,
        "total_ms": total_ms,
        "avg_ms": total_ms / iterations.max(1) as f64,
        "throughput_mib_s": if elapsed_secs > 0.0 {
            (bytes as f64 / (1024.0 * 1024.0)) / elapsed_secs
        } else {
            0.0
        },
        "bytes": bytes,
        "dirty_rows": dirty_rows,
    })
}

fn generate_ls_like_burst(seed: usize, lines: usize) -> Vec<u8> {
    let mut out = String::with_capacity(lines * 96);
    for i in 0..lines {
        let n = seed * lines + i;
        let kind = if n % 5 == 0 { 'd' } else { '-' };
        let size = 1024 + (n * 37) % 2_000_000;
        let month = ["Jan", "Feb", "Mar", "Apr", "May", "Jun"][n % 6];
        let color_prefix = if kind == 'd' { "\x1b[34m" } else { "\x1b[0m" };
        out.push_str(color_prefix);
        out.push_str(&format!(
            "{kind}rw-r--r--  1 user  staff {:>8} {month} {:>2} {:>5} file_{:05}.txt\x1b[0m\r\n",
            size,
            (n % 28) + 1,
            "12:34",
            n
        ));
    }
    out.into_bytes()
}

fn generate_line_flood(start_idx: usize, lines: usize) -> Vec<u8> {
    let mut out = String::with_capacity(lines * 80);
    for i in 0..lines {
        let n = start_idx + i;
        out.push_str(&format!(
            "line {:06} | {:08x} {:08x} {:08x} {:08x}\r\n",
            n,
            n.wrapping_mul(17),
            n.wrapping_mul(29),
            n.wrapping_mul(43),
            n.wrapping_mul(59)
        ));
    }
    out.into_bytes()
}

fn generate_ctrl_l_clear_payload(seed: usize) -> Vec<u8> {
    // Typical clear-screen style: home + clear visible + redraw prompt line.
    format!(
        "\x1b[H\x1b[2J\x1b[3J\x1b[0muser@host ~/workdir % echo {}\r\n",
        seed
    )
    .into_bytes()
}

fn normalize_sel(a: (u16, u16), b: (u16, u16)) -> ((u16, u16), (u16, u16)) {
    if a.1 < b.1 || (a.1 == b.1 && a.0 <= b.0) {
        (a, b)
    } else {
        (b, a)
    }
}

fn scan_selection_region(grid: &[GridLine], start: (u16, u16), end: (u16, u16)) -> (usize, u64) {
    let mut cells = 0usize;
    let mut checksum = 0u64;
    for row in start.1..=end.1 {
        let Some(line) = grid.get(row as usize) else {
            break;
        };
        let col_start = if row == start.1 { start.0 as usize } else { 0 };
        let col_end = if row == end.1 {
            (end.0 as usize + 1).min(line.cells.len())
        } else {
            line.cells.len()
        };
        for cell in &line.cells[col_start..col_end] {
            cells += 1;
            checksum = checksum
                .wrapping_add(cell.c as u32 as u64)
                .wrapping_add(cell.fg.r as u64)
                .wrapping_add(cell.bg.b as u64);
        }
    }
    (cells, checksum)
}
