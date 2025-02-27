use eframe::egui;
use serialport::SerialPort;
use std::{sync::{mpsc, Arc, Mutex}, thread, time::Duration};
use std::process::Command;
use std::os::unix::process::CommandExt;
use eframe::egui::plot::{Line, Plot, PlotPoints};

struct SerialMonitorApp {
    available_ports: Vec<String>,
    selected_port: String,
    baud_rate: u32,
    received_data: String,
    port: Option<Arc<Mutex<Box<dyn SerialPort>>>>,
    rx: Option<mpsc::Receiver<String>>,
    send_data: String,
    is_hex_input: bool,
    is_hex_display: bool,
    common_baud_rates: Vec<u32>,
    plot_window: PlotWindow,
}

impl Default for SerialMonitorApp {
    fn default() -> Self {
        let ports = serialport::available_ports()
            .unwrap_or_default()
            .into_iter()
            .map(|p| p.port_name)
            .collect();

        Self {
            available_ports: ports,
            selected_port: String::new(),
            baud_rate: 115200,  // 改为更常用的默认值
            received_data: String::new(),
            port: None,
            rx: None,
            send_data: String::new(),
            is_hex_input: false,
            is_hex_display: false,
            common_baud_rates: vec![1200, 2400, 4800, 9600, 19200, 38400, 57600, 115200],
            plot_window: PlotWindow::default(),
        }
    }
}

impl eframe::App for SerialMonitorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(rx) = &self.rx {
            if let Ok(data) = rx.try_recv() {
                if self.is_hex_display {
                    let hex_str: String = data.bytes()
                        .map(|b| format!("{:02X} ", b))
                        .collect();
                    self.received_data.push_str(&hex_str);
                } else {
                    self.received_data.push_str(&data);
                }
                self.received_data.push('\n');
                self.parse_data(&data); // 添加数据解析
            }
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Serial Monitor");
            
            ui.horizontal(|ui| {
                egui::ComboBox::from_label("Port")
                    .selected_text(&self.selected_port)
                    .width(200.0)  // 使用 width 替代 min_width
                    .show_ui(ui, |ui| {
                        for port in &self.available_ports {
                            ui.selectable_value(&mut self.selected_port, port.clone(), port);
                        }
                    });
                
                if ui.button("Refresh").clicked() {
                    self.available_ports = serialport::available_ports()
                        .unwrap_or_default()
                        .into_iter()
                        .map(|p| p.port_name)
                        .collect();
                }
            });

            ui.horizontal(|ui| {
                ui.label("Rate:");
                egui::ComboBox::from_label("")
                    .selected_text(self.baud_rate.to_string())
                    .show_ui(ui, |ui| {
                        for &rate in &self.common_baud_rates {
                            ui.selectable_value(&mut self.baud_rate, rate, rate.to_string());
                        }
                    });
                
                // 保留手动输入功能
                ui.add(egui::DragValue::new(&mut self.baud_rate)
                    .speed(100)
                    .clamp_range(1200..=115200));
            });

            if ui.button(if self.port.is_some() { "Stop" } else { "Start" }).clicked() {
                if self.port.is_none() {
                    self.connect();
                } else {
                    self.disconnect();
                }
            }

            ui.separator();

            // Add send data controls
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.is_hex_input, "HEX_Input");
                ui.checkbox(&mut self.is_hex_display, "HEX_Display");
            });

            ui.horizontal(|ui| {
                let text_edit = ui.text_edit_singleline(&mut self.send_data);
                if ui.button("Send").clicked() && self.port.is_some() {
                    self.send_data();
                }
                if text_edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    self.send_data();
                }
            });

            ui.separator();

            let text_height = ui.available_height() - 60.0;
            egui::ScrollArea::vertical()
                .max_height(text_height)
                .stick_to_bottom(true)  // 添加这一行来保持滚动到底部
                .show(ui, |ui| {
                    ui.add_sized(
                        [ui.available_width(), text_height],
                        egui::TextEdit::multiline(&mut self.received_data)
                            .desired_rows(10)
                            .lock_focus(true)
                    );
                });

            ui.horizontal(|ui| {
                if ui.button("Open Plot").clicked() {
                    self.plot_window.is_open = true;
                }
                if ui.button("Clean").clicked() {
                    self.received_data.clear();
                }
                if ui.button("Reset MCU").clicked() && self.port.is_some() {
                    self.reset_device();
                }
            });
        });

        // 更新绘图窗口
        self.update_plots(ctx);

        ctx.request_repaint();
    }
}

impl SerialMonitorApp {
    fn connect(&mut self) {
        if self.selected_port.is_empty() {
            self.received_data = "Select one port.".to_string();
            return;
        }

        match serialport::new(&self.selected_port, self.baud_rate)
            .timeout(Duration::from_millis(10))
            .data_bits(serialport::DataBits::Eight)
            .flow_control(serialport::FlowControl::None)
            .parity(serialport::Parity::None)
            .stop_bits(serialport::StopBits::One)
            .open()
        {
            Ok(mut port) => {
                // Set DTR and RTS after port is opened
                let _ = port.write_data_terminal_ready(false);
                let _ = port.write_request_to_send(false);
                
                let port = Arc::new(Mutex::new(port));
                let port_clone = Arc::clone(&port);
                let (tx, rx) = mpsc::channel();

                thread::spawn(move || {
                    let mut serial_buf: Vec<u8> = vec![0; 1024];
                    loop {
                        if let Ok(mut port) = port_clone.lock() {
                            match port.read(serial_buf.as_mut_slice()) {
                                Ok(t) => {
                                    if t > 0 {
                                        let s = String::from_utf8_lossy(&serial_buf[..t]).into_owned();
                                        let _ = tx.send(s);
                                    }
                                }
                                Err(ref e) if e.kind() == std::io::ErrorKind::TimedOut => {
                                    continue;
                                }
                                Err(e) => {
                                    let _ = tx.send(format!("Error: {}\n", e));
                                    thread::sleep(Duration::from_millis(100));
                                    let _ = port.clear(serialport::ClearBuffer::All);
                                }
                            }
                        }
                        thread::sleep(Duration::from_millis(10));
                    }
                });

                if let Ok(mut port) = port.lock() {
                    let _ = port.clear(serialport::ClearBuffer::All);
                }

                self.port = Some(port);
                self.rx = Some(rx);
                self.received_data = "Connected\n".to_string();
            }
            Err(e) => {
                self.received_data = format!("Failed: {}\n", e);
            }
        }
    }

    fn disconnect(&mut self) {
        self.port = None;
        self.rx = None;
        self.received_data.push_str("Unconnected\n");
    }

    fn send_data(&mut self) {
        if let Some(port) = &self.port {
            let data = if self.is_hex_input {
                // 转换hex字符串为字节
                let hex_str = self.send_data.replace(" ", "");
                let mut bytes = Vec::new();
                for i in (0..hex_str.len()).step_by(2) {
                    if i + 2 <= hex_str.len() {
                        if let Ok(byte) = u8::from_str_radix(&hex_str[i..i + 2], 16) {
                            bytes.push(byte);
                        }
                    }
                }
                bytes
            } else {
                self.send_data.as_bytes().to_vec()
            };

            if let Ok(mut port) = port.lock() {
                if port.write(&data).is_ok() {
                    if self.is_hex_display {
                        let hex_str: String = data.iter()
                            .map(|b| format!("{:02X} ", b))
                            .collect();
                        self.received_data.push_str(&format!("Send: {}\n", hex_str));
                    } else {
                        self.received_data.push_str(&format!("Send: {}\n", self.send_data));
                    }
                }
            }
            self.send_data.clear();
        }
    }

    fn parse_data(&mut self, line: &str) {
        if !self.plot_window.is_open || self.plot_window.is_paused {  // 检查暂停状态
            return;
        }

        println!("Trying to parse line: {}", line);
        let line = line.trim();
        let mut values = Vec::new();

        if line.starts_with("Pace: FL:") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            for part in parts {
                if let Ok(value) = part.parse::<f64>() {
                    values.push(value);
                }
            }
        }

        println!("Found {} values: {:?}", values.len(), values);

        if values.len() == 4 {
            for (i, &value) in values.iter().enumerate() {
                self.plot_window.plot_data[i].push(value, self.plot_window.max_points);
                println!("Updated plot {} with value {}", i, value);
            }
        }
    }

    fn update_plots(&mut self, ctx: &egui::Context) {
        if !self.plot_window.is_open {
            return;
        }

        egui::Window::new("Data Plot")
            .open(&mut self.plot_window.is_open)
            .default_size([800.0, 800.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Format:");
                    ui.text_edit_singleline(&mut self.plot_window.format);
                    ui.separator();
                    ui.label("Max Points:");
                    ui.add(egui::DragValue::new(&mut self.plot_window.max_points)
                        .speed(10)
                        .clamp_range(100..=10000));
                    if ui.button(if self.plot_window.is_paused { "Resume" } else { "Pause" }).clicked() {
                        self.plot_window.is_paused = !self.plot_window.is_paused;
                    }
                    if ui.button("Clean Plot").clicked() {
                        for plot_data in &mut self.plot_window.plot_data {
                            plot_data.values.clear();
                            plot_data.times.clear();
                            plot_data.start_time = None;
                        }
                    }
                });
                
                // 添加暂停状态显示
                if self.plot_window.is_paused {
                    ui.label(egui::RichText::new("PAUSED").color(egui::Color32::RED));
                }

                // 添加调试信息显示
                for i in 0..4 {
                    ui.label(format!(
                        "{}: {} points", 
                        self.plot_window.names[i],
                        self.plot_window.plot_data[i].values.len()
                    ));
                }

                ui.separator();

                let available_width = ui.available_width();  // 获取可用宽度
                for i in 0..4 {
                    Plot::new(self.plot_window.names[i])
                        .height(150.0)
                        .width(available_width)  // 设置宽度为可用宽度
                        .show_axes([true, true])
                        .include_y(0.0)
                        .show(ui, |plot_ui| {
                            plot_ui.line(Line::new(
                                self.plot_window.plot_data[i].get_points()
                            ).width(2.0));
                        });
                }
            });
    }

    fn reset_device(&mut self) {
        if let Some(port) = &self.port {
            if let Ok(mut port) = port.lock() {
                // Reset sequence
                let _ = port.clear(serialport::ClearBuffer::All);
                let _ = port.write_data_terminal_ready(true);
                thread::sleep(Duration::from_millis(100));
                let _ = port.write_data_terminal_ready(false);
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

// 新增的数据结构
#[derive(Default)]
struct PlotData {
    values: Vec<f64>,
    times: Vec<f64>,
    start_time: Option<std::time::Instant>,
}

impl PlotData {
    fn push(&mut self, value: f64, max_points: usize) {
        if self.start_time.is_none() {
            self.start_time = Some(std::time::Instant::now());
        }
        let time = self.start_time.unwrap().elapsed().as_secs_f64();
        self.times.push(time);
        self.values.push(value);
        
        // Remove oldest points if exceeding max_points
        if self.values.len() > max_points {
            self.values.remove(0);
            self.times.remove(0);
        }
    }

    fn get_points(&self) -> PlotPoints {
        self.times.iter()
            .zip(self.values.iter())
            .map(|(&x, &y)| [x, y])
            .collect()
    }
}

struct PlotWindow {
    is_open: bool,
    format: String,
    plot_data: [PlotData; 4],
    names: [&'static str; 4],
    max_points: usize,
    is_paused: bool,  // 添加暂停状态
}

impl Default for PlotWindow {
    fn default() -> Self {
        Self {
            is_open: false,
            format: String::from("Pace: FL: %% FR: %% RL: %% RR: %%"),
            plot_data: Default::default(),
            names: ["FL", "FR", "RL", "RR"],
            max_points: 1000,
            is_paused: false,  // 初始不暂停
        }
    }
}

fn is_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

fn restart_with_sudo() -> ! {
    let args: Vec<String> = std::env::args().collect();
    let err = Command::new("sudo")
        .args(&[&args[0]])
        .args(&args[1..])
        .exec();
    eprintln!("Failed to execute sudo: {}", err);
    std::process::exit(1);
}

fn main() -> Result<(), eframe::Error> {
// Check for root privileges
    if !is_root() {
        println!("Serial Monitor needs root privileges to access serial ports.");
        println!("Restarting with sudo...");
        restart_with_sudo();
    }

    let options = eframe::NativeOptions {
        initial_window_size: Some(egui::vec2(600.0, 400.0)),
        ..Default::default()
    };

    eframe::run_native(
        "Serial Monitor",
        options,
        Box::new(|_cc| Box::new(SerialMonitorApp::default())),
    )
}
