//! 软件 VP8 编解码器实现
//!
//! 基于 libvpx 实现纯软件 VP8 编码/解码，用于无硬件编解码能力的设备
//!
//! ## 架构
//! - SoftwareVP8Encoder: I420帧 -> VP8 比特流
//! - SoftwareVP8Decoder: VP8 比特流 -> I420帧
//!
//! ## 使用场景
//! - OHOS 设备不支持任何视频硬件编码
//! - 需要兼容 VP8 的 WebRTC 通话

use std::collections::VecDeque;
use std::ptr;
use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};
use std::ffi::c_int;

use super::libvpx_ffi::*;

// ============================================================
// 编码输出帧
// ============================================================

/// 软件编码输出帧
#[derive(Clone)]
pub struct SoftwareEncodedFrame {
    pub data: Vec<u8>,
    pub timestamp_us: i64,
    pub is_key_frame: bool,
}

// ============================================================
// VP8 软件编码器
// ============================================================

/// VP8 软件编码器配置
pub struct SoftwareVP8EncoderConfig {
    pub width: u32,
    pub height: u32,
    pub frame_rate: u32,
    pub bit_rate: u64,  // bps
    pub keyframe_interval: u32,  // 帧数
}

impl Default for SoftwareVP8EncoderConfig {
    fn default() -> Self {
        Self {
            width: 480,  // v27: 从 640 降到 480，减少带宽和编码压力
            height: 360,
            frame_rate: 30,
            bit_rate: 500_000,  // v27: 从 1Mbps 降到 500kbps，控制关键帧大小
            keyframe_interval: 3,  // v27: 从 5 降到 3，更快恢复，每 0.1 秒一个关键帧
        }
    }
}

/// VP8 软件编码器
pub struct SoftwareVP8Encoder {
    ctx: vpx_codec_ctx_t,
    config: SoftwareVP8EncoderConfig,
    image: vpx_image_t,
    output_queue: VecDeque<SoftwareEncodedFrame>,
    is_initialized: AtomicBool,
    frame_count: u64,
    /// v18: 距上次关键帧的帧计数，用于强制关键帧
    frames_since_keyframe: u32,
    /// v20: 强制下一帧为关键帧的标志
    force_next_frame_keyframe: bool,
}

// VP8 编码器上下文需要手动释放
unsafe impl Send for SoftwareVP8Encoder {}

impl SoftwareVP8Encoder {
    /// 创建 VP8 软件编码器
    pub fn new(config: SoftwareVP8EncoderConfig) -> Self {
        let mut ctx = vpx_codec_ctx_t {
            name: ptr::null(),
            iface: ptr::null_mut(),
            err: 0,
            err_detail: ptr::null(),
            init_flags: 0,
            config: vpx_codec_ctx_config { raw: ptr::null() },
            priv_: ptr::null_mut(),
        };
        
        let mut image = vpx_image_t {
            fmt: 0,
            cs: 0,
            range: 0,
            w: 0,
            h: 0,
            bit_depth: 0,
            d_w: 0,
            d_h: 0,
            r_w: 0,
            r_h: 0,
            x_chroma_shift: 0,
            y_chroma_shift: 0,
            planes: [ptr::null_mut(); 4],
            stride: [0; 4],
            bps: 0,
            user_priv: ptr::null_mut(),
            img_data: ptr::null_mut(),
            img_data_owner: 0,
            self_allocd: 0,
            fb_priv: ptr::null_mut(),
        };
        
        Self {
            ctx,
            config,
            image,
            output_queue: VecDeque::new(),
            is_initialized: AtomicBool::new(false),
            frame_count: 0,
            frames_since_keyframe: 0,
            force_next_frame_keyframe: false,
        }
    }
    
    /// 初始化编码器
    pub fn initialize(&mut self) -> bool {
        log::info!("[SW-VP8Enc] 初始化: {}x{} @ {}fps, {}bps",
            self.config.width, self.config.height, self.config.frame_rate, self.config.bit_rate);
        
        unsafe {
            // 打印 libvpx 版本信息
            let ver = vpx_codec_version();
            let ver_str = vpx_codec_version_str();
            if !ver_str.is_null() {
                let vs = std::ffi::CStr::from_ptr(ver_str);
                log::info!("[SW-VP8Enc] libvpx version: {:?} (0x{:x})", vs, ver);
            }
            
            // 获取 VP8 编码器接口
            let iface = vpx_codec_vp8_cx();
            if iface.is_null() {
                log::error!("[SW-VP8Enc] 获取 VP8 编码器接口失败");
                return false;
            }
            
            // 获取默认配置
            let mut cfg: vpx_codec_enc_cfg_t = std::mem::zeroed();
            let cfg_result = vpx_codec_enc_config_default(iface, &mut cfg, 0);
            if cfg_result != VPX_CODEC_OK {
                log::error!("[SW-VP8Enc] 获取默认配置失败: err={}", cfg_result);
                return false;
            }
            
            log::info!("[SW-VP8Enc] 默认配置: g_w={}, g_h={}, rc_target_bitrate={}, kf_mode={}",
                cfg.g_w, cfg.g_h, cfg.rc_target_bitrate, cfg.kf_mode);
            
            // 只修改必要的参数，其他保留默认值
            cfg.g_w = self.config.width;
            cfg.g_h = self.config.height;
            cfg.g_timebase = vpx_rational_t { num: 1, den: self.config.frame_rate as i32 };
            cfg.rc_target_bitrate = (self.config.bit_rate / 1000) as u32;  // kbps
            cfg.g_error_resilient = 1;  // 启用错误恢复 (WebRTC 需要)
            cfg.g_lag_in_frames = 0;    // 零延迟 (实时编码)
            cfg.rc_end_usage = VPX_CBR;  // CBR 码率控制 (实时编码推荐)
            cfg.rc_buf_sz = 300;         // v15: 300ms buffer (减少延迟)
            cfg.rc_buf_initial_sz = 100;  // v15: 100ms 初始 buffer (快速启动)
            cfg.rc_buf_optimal_sz = 200;  // v15: 200ms 最优 buffer
            cfg.kf_mode = VPX_KF_AUTO;
            cfg.kf_min_dist = 0;  // 允许任意位置的关键帧
            cfg.kf_max_dist = self.config.keyframe_interval;
            
            log::info!("[SW-VP8Enc] 编码器配置: {}x{}, {}kbps, g_timebase={}/{}, kf_max_dist={}, rc_end_usage={}, g_lag={}",
                cfg.g_w, cfg.g_h, cfg.rc_target_bitrate,
                cfg.g_timebase.num, cfg.g_timebase.den,
                cfg.kf_max_dist, cfg.rc_end_usage, cfg.g_lag_in_frames);
            
            // 初始化编码器
            let init_result = vpx_codec_enc_init(&mut self.ctx, iface, &cfg, 0);
            if init_result != VPX_CODEC_OK {
                let err = vpx_codec_error(&self.ctx);
                if !err.is_null() {
                    let err_str = std::ffi::CStr::from_ptr(err);
                    log::error!("[SW-VP8Enc] 初始化失败({}): {:?}", init_result, err_str);
                }
                return false;
            }
            
            log::info!("[SW-VP8Enc] vpx_codec_enc_init 成功");
            
            // 设置编码速度 (CPU used)
            // VP8 范围: -16 到 16, 正值越大=越快/越低质量
            // 实时视频: 8-16
            // v19: 最大速度 16 — OHOS 设备主线程必须赢得喘息空间
            // VP8 编码阻塞主线程，cpu_used=10 时 ~80ms/帧，
            // cpu_used=16 可减少到40-50ms/帧，节省 ~300ms/s 主线程时间
            let cpu_used: c_int = 16;  // v19: 最快编码速度
            vpx_codec_control_(&mut self.ctx, VP8E_SET_CPUUSED, cpu_used);
            log::info!("[SW-VP8Enc] VP8E_SET_CPUUSED={}", cpu_used);
            
            // 禁用自动 AltRef (减少延迟)
            let autoaltref: c_int = 0;
            vpx_codec_control_(&mut self.ctx, VP8E_SET_ENABLEAUTOALTREF, autoaltref);
            
            // 设置噪声敏感度
            let noise_sensitivity: c_int = 1;
            vpx_codec_control_(&mut self.ctx, VP8E_SET_NOISE_SENSITIVITY, noise_sensitivity);
            
            // 分配图像缓冲区
            if vpx_img_alloc(&mut self.image, VPX_IMG_FMT_I420, 
                self.config.width, self.config.height, 1).is_null() {
                log::error!("[SW-VP8Enc] 分配图像缓冲区失败");
                return false;
            }
            
            log::info!("[SW-VP8Enc] 初始化成功, image planes: Y={}, U={}, V={}",
                self.image.planes[0] as usize, self.image.planes[1] as usize, self.image.planes[2] as usize);
        }
        
        self.is_initialized.store(true, Ordering::SeqCst);
        true
    }
    
    /// 编码一帧 I420 数据
    /// 
    /// @param yuv_data YUV 数据 (I420 或 NV12 格式)
    /// @param timestamp_us 时间戳（微秒）
    /// @param is_nv12 是否为 NV12 格式 (默认为 I420)
    /// @return 是否成功
    pub fn encode(&mut self, yuv_data: &[u8], timestamp_us: i64) -> bool {
        self.encode_with_format(yuv_data, timestamp_us, false)
    }
    
    /// 编码一帧 NV12 数据
    /// 
    /// @param nv12_data NV12 格式的 YUV 数据
    /// @param timestamp_us 时间戳（微秒）
    /// @return 是否成功
    pub fn encode_nv12(&mut self, nv12_data: &[u8], timestamp_us: i64) -> bool {
        self.encode_with_format(nv12_data, timestamp_us, true)
    }
    
    /// 编码一帧 YUV 数据 (支持 I420 和 NV12 格式)
    fn encode_with_format(&mut self, yuv_data: &[u8], timestamp_us: i64, is_nv12: bool) -> bool {
        if !self.is_initialized.load(Ordering::SeqCst) {
            log::warn!("[SW-VP8Enc] 编码器未初始化");
            return false;
        }
        
        // 第一帧打印详细诊断信息
        if self.frame_count == 0 {
            log::info!("[SW-VP8Enc] 第一帧: data_len={}, config={}x{}, expected={}, is_nv12={}, y_stride={}, u_stride={}, v_stride={}",
                yuv_data.len(), self.config.width, self.config.height,
                (self.config.width * self.config.height) as usize + (self.config.width * self.config.height) as usize / 2,
                is_nv12,
                self.image.stride[0], self.image.stride[1], self.image.stride[2]);
        }
        
        // 使用 catch_unwind 防止任何 panic 导致 abort
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.encode_with_format_inner(yuv_data, timestamp_us, is_nv12)
        }));
        
        match result {
            Ok(success) => success,
            Err(_) => {
                log::warn!("[SW-VP8Enc] 编码过程中发生 panic，已捕获");
                false
            }
        }
    }
    
    /// 编码内部实现（被 catch_unwind 包裹）
    fn encode_with_format_inner(&mut self, yuv_data: &[u8], timestamp_us: i64, is_nv12: bool) -> bool {
        
        // 严格检查输入数据长度
        let y_size = (self.config.width * self.config.height) as usize;
        let expected_size = y_size + y_size / 2;  // NV12/I420: Y + UV
        if yuv_data.len() < expected_size {
            log::warn!("[SW-VP8Enc] 帧数据过小");
            return false;
        }
        
        unsafe {
            let u_size = y_size / 4;
            
            // 检查 image planes 是否有效
            if self.image.planes[0].is_null() || self.image.planes[1].is_null() || self.image.planes[2].is_null() {
                log::error!("[SW-VP8Enc] image planes 为空");
                return false;
            }
            
            // 检查 stride 是否有效
            let y_stride = self.image.stride[0] as usize;
            let u_stride = self.image.stride[1] as usize;
            let v_stride = self.image.stride[2] as usize;
            
            // Y 平面 (I420 和 NV12 相同) - 按行复制避免 stride 不匹配问题
            if y_stride == self.config.width as usize {
                // stride 匹配，直接复制
                ptr::copy_nonoverlapping(
                    yuv_data.as_ptr(),
                    self.image.planes[0],
                    y_size,
                );
            } else {
                // stride 不匹配，按行复制
                for row in 0..self.config.height as usize {
                    ptr::copy_nonoverlapping(
                        yuv_data.as_ptr().add(row * self.config.width as usize),
                        self.image.planes[0].add(row * y_stride),
                        self.config.width as usize,
                    );
                }
            }
            
            if is_nv12 {
                // NV12 格式: UV 交织为 UVUVUV...
                // 需要分离为 U 平面和 V 平面
                let uv_data = &yuv_data[y_size..];
                let uv_width = (self.config.width / 2) as usize;
                let uv_height = (self.config.height / 2) as usize;
                let uv_data_len = uv_data.len();
                
                // 预检查 UV 数据长度是否匹配
                let expected_uv_len = uv_width * uv_height * 2;
                if uv_data_len < expected_uv_len {
                    log::warn!("[SW-VP8Enc] NV12 UV 数据长度不匹配");
                    return false;
                }
                
                // 检查 stride 是否有效
                let u_stride = self.image.stride[1] as usize;
                let v_stride = self.image.stride[2] as usize;
                if u_stride < uv_width || v_stride < uv_width {
                    log::warn!("[SW-VP8Enc] stride 过小");
                    return false;
                }
                
                // 检查目标缓冲区边界
                let u_plane_size = uv_height * u_stride;
                let v_plane_size = uv_height * v_stride;
                // 简单检查：只要能写入第一行第一个像素和最后一行最后一个像素即可
                if u_plane_size == 0 || v_plane_size == 0 {
                    log::warn!("[SW-VP8Enc] UV 平面大小为0");
                    return false;
                }
                
                for row in 0..uv_height {
                    for col in 0..uv_width {
                        // 使用 uv_width 而非 config.width 计算索引
                        let uv_idx = row * uv_width * 2 + col * 2;
                        // 额外边界检查
                        if uv_idx + 1 >= uv_data_len {
                            log::warn!("[SW-VP8Enc] UV 索引越界");
                            return false;
                        }
                        let u_val = uv_data[uv_idx];
                        let v_val = uv_data[uv_idx + 1];
                        
                        // 计算目标地址
                        let u_offset = row * u_stride + col;
                        let v_offset = row * v_stride + col;
                        if u_offset >= u_plane_size || v_offset >= v_plane_size {
                            log::warn!("[SW-VP8Enc] 目标偏移越界");
                            return false;
                        }
                        
                        // U 平面
                        *self.image.planes[1].add(u_offset) = u_val;
                        // V 平面
                        *self.image.planes[2].add(v_offset) = v_val;
                    }
                }
            } else {
                // I420 格式: 按行复制处理 stride 不匹配
                let uv_height = (self.config.height / 2) as usize;
                let uv_width = (self.config.width / 2) as usize;
                
                // U 平面
                if u_stride == uv_width {
                    ptr::copy_nonoverlapping(
                        yuv_data.as_ptr().add(y_size),
                        self.image.planes[1],
                        u_size,
                    );
                } else {
                    for row in 0..uv_height {
                        ptr::copy_nonoverlapping(
                            yuv_data.as_ptr().add(y_size + row * uv_width),
                            self.image.planes[1].add(row * u_stride),
                            uv_width,
                        );
                    }
                }
                
                // V 平面
                if v_stride == uv_width {
                    ptr::copy_nonoverlapping(
                        yuv_data.as_ptr().add(y_size + u_size),
                        self.image.planes[2],
                        u_size,
                    );
                } else {
                    for row in 0..uv_height {
                        ptr::copy_nonoverlapping(
                            yuv_data.as_ptr().add(y_size + u_size + row * uv_width),
                            self.image.planes[2].add(row * v_stride),
                            uv_width,
                        );
                    }
                }
            }
            
            // v18: 强制关键帧 — 不依赖 kf_max_dist (CBR 模式下可能跳过)
            let force_kf = self.frame_count == 0 
                || self.frames_since_keyframe >= self.config.keyframe_interval
                || self.force_next_frame_keyframe;
            
            // 重置强制标志
            if self.force_next_frame_keyframe {
                log::info!("[SW-VP8Enc] 执行强制关键帧请求");
                self.force_next_frame_keyframe = false;
            }

            // Log first 5 frames' keyframe decisions for diagnostics
            if self.frame_count < 5 {
                log::info!("[SW-VP8Enc] frame #{} force_kf={} since_kf={} kf_interval={}",
                    self.frame_count, force_kf, self.frames_since_keyframe, self.config.keyframe_interval);
            }
            
            let flags: vpx_enc_frame_flags_t = if force_kf {
                VPX_EFLAG_FORCE_KF
            } else {
                0
            };
            
            // 编码
            let pts = self.frame_count as vpx_codec_pts_t;
            let result = vpx_codec_encode(
                &mut self.ctx,
                &self.image,
                pts,
                1,  // duration
                flags,
                VPX_DL_REALTIME,  // libvpx 编译时使用了 --enable-realtime-only，必须用 REALTIME deadline
            );
            
            if result != VPX_CODEC_OK {
                let err = vpx_codec_error(&self.ctx);
                if !err.is_null() {
                    let err_str = std::ffi::CStr::from_ptr(err);
                    log::warn!("[SW-VP8Enc] 编码失败: {:?}", err_str);
                }
                return false;
            }
            
            // 获取编码输出
            let mut iter: vpx_codec_iter_t = ptr::null();
            loop {
                let pkt = vpx_codec_get_cx_data(
                    &mut self.ctx as *mut vpx_codec_ctx_t,
                    &mut iter as *mut vpx_codec_iter_t
                );
                if pkt.is_null() {
                    break;
                }
                
                if let Some((data, size, is_key_frame)) = extract_frame_from_pkt(pkt) {
                    let mut frame_data = vec![0u8; size];
                    ptr::copy_nonoverlapping(data, frame_data.as_mut_ptr(), size);
                    
                    // Log first 10 output frames at INFO to verify keyframe generation
                    if self.frame_count < 10 {
                        log::info!("[SW-VP8Enc] 输出帧 #{}: {} bytes, key={}, head=[{:02x} {:02x} {:02x} {:02x}]",
                            self.frame_count, size, is_key_frame,
                            frame_data[0], frame_data.get(1).copied().unwrap_or(0),
                            frame_data.get(2).copied().unwrap_or(0), frame_data.get(3).copied().unwrap_or(0));
                    } else {
                        log::debug!("[SW-VP8Enc] 输出帧: {} bytes, key={}", size, is_key_frame);
                    }
                    
                    self.output_queue.push_back(SoftwareEncodedFrame {
                        data: frame_data,
                        timestamp_us,
                        is_key_frame,
                    });
                }
            }
        }
        
        self.frame_count += 1;
        self.frames_since_keyframe += 1;
        // 关键帧输出时重置计数器
        if self.output_queue.back().map_or(false, |f| f.is_key_frame) {
            self.frames_since_keyframe = 0;
        }
        true
    }
    
    /// 轮询编码输出
    pub fn poll_output(&mut self) -> Option<SoftwareEncodedFrame> {
        self.output_queue.pop_front()
    }
    
    /// 请求关键帧
    pub fn request_keyframe(&mut self) -> bool {
        if !self.is_initialized.load(Ordering::SeqCst) {
            return false;
        }
        
        // 设置标志，下一帧会被强制编码为关键帧
        self.force_next_frame_keyframe = true;
        log::info!("[SW-VP8Enc] 已请求关键帧（下一帧将强制为关键帧）");
        true
    }
    
    /// 获取编码器统计
    pub fn get_stats(&self) -> (u64, u64) {
        (self.frame_count, self.output_queue.len() as u64)
    }
}

impl Drop for SoftwareVP8Encoder {
    fn drop(&mut self) {
        if self.is_initialized.load(Ordering::SeqCst) {
            unsafe {
                vpx_img_free(&mut self.image);
                vpx_codec_destroy(&mut self.ctx);
            }
            log::info!("[SW-VP8Enc] 已销毁");
        }
    }
}

// ============================================================
// VP8 软件解码器
// ============================================================

/// VP8 软件解码器配置
pub struct SoftwareVP8DecoderConfig {
    pub width: u32,
    pub height: u32,
}

impl Default for SoftwareVP8DecoderConfig {
    fn default() -> Self {
        Self {
            width: 640,
            height: 480,
        }
    }
}

/// 解码输出帧
#[derive(Clone)]
pub struct DecodedFrame {
    /// I420 格式数据
    pub i420_data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub timestamp_us: i64,
}

/// VP8 软件解码器
pub struct SoftwareVP8Decoder {
    ctx: vpx_codec_ctx_t,
    config: SoftwareVP8DecoderConfig,
    output_queue: VecDeque<DecodedFrame>,
    is_initialized: AtomicBool,
    /// 解码失败计数（用于限制日志输出）
    decode_error_count: u32,
    dec_output_scratch: Vec<u8>,
    /// 成功解码帧计数（用于诊断日志）
    decode_ok_count: u64,
}

unsafe impl Send for SoftwareVP8Decoder {}

impl SoftwareVP8Decoder {
    /// 创建 VP8 软件解码器
    pub fn new(config: SoftwareVP8DecoderConfig) -> Self {
        let ctx = vpx_codec_ctx_t {
            name: ptr::null(),
            iface: ptr::null_mut(),
            err: 0,
            err_detail: ptr::null(),
            init_flags: 0,
            config: vpx_codec_ctx_config { raw: ptr::null() },
            priv_: ptr::null_mut(),
        };
        
        Self {
            ctx,
            config,
            output_queue: VecDeque::new(),
            is_initialized: AtomicBool::new(false),
            decode_error_count: 0,
            dec_output_scratch: Vec::new(),
            decode_ok_count: 0,
        }
    }
    
    /// 初始化解码器
    pub fn initialize(&mut self) -> bool {
        log::info!("[SW-VP8Dec] 初始化: {}x{}", self.config.width, self.config.height);
        
        unsafe {
            // 获取 VP8 解码器接口
            let iface = vpx_codec_vp8_dx();
            if iface.is_null() {
                log::error!("[SW-VP8Dec] 获取 VP8 解码器接口失败");
                return false;
            }
            
            // 创建解码器配置
            // w=0, h=0 让 libvpx 从码流中自动检测分辨率，避免预分配缓冲区
            // 大小与实际视频不匹配导致的解码错误。
            let cfg = vpx_codec_dec_cfg_t {
                threads: 1,
                w: 0,  // auto-detect from bitstream
                h: 0,  // auto-detect from bitstream
            };
            
            // 初始化解码器
            if vpx_codec_dec_init(&mut self.ctx, iface, &cfg, 0) != VPX_CODEC_OK {
                let err = vpx_codec_error(&self.ctx);
                if !err.is_null() {
                    let err_str = std::ffi::CStr::from_ptr(err);
                    log::error!("[SW-VP8Dec] 初始化失败: {:?}", err_str);
                }
                return false;
            }
            
            log::info!("[SW-VP8Dec] 初始化成功");
        }
        
        self.is_initialized.store(true, Ordering::SeqCst);
        true
    }
    
    /// 解码一帧 VP8 数据
    /// 
    /// @param vp8_data VP8 编码数据
    /// @param timestamp_us 时间戳（微秒）
    /// @return 是否成功
    pub fn decode(&mut self, vp8_data: &[u8], timestamp_us: i64) -> bool {
        if !self.is_initialized.load(Ordering::SeqCst) {
            log::warn!("[SW-VP8Dec] 解码器未初始化");
            return false;
        }
        
        if vp8_data.is_empty() {
            log::warn!("[SW-VP8Dec] 空数据");
            return false;
        }
        
        // VP8 解码器在收到关键帧之前会返回错误，不会破坏内部状态。
        // 直接送入解码器，等关键帧到来自然成功。
        
        unsafe {
            // 解码
            let result = vpx_codec_decode(
                &mut self.ctx,
                vp8_data.as_ptr(),
                vp8_data.len() as u32,
                ptr::null_mut(),
                0,  // 无限期等待
            );
            
            if result != VPX_CODEC_OK {
                // 解码失败（常见原因：未收到关键帧就收到 P 帧）
                // libvpx 不会破坏内部状态，等关键帧到来就会恢复
                self.decode_error_count += 1;
                if self.decode_error_count <= 3 || self.decode_error_count % 100 == 0 {
                    let err = vpx_codec_error(&self.ctx);
                    let first_bytes: String = vp8_data.iter().take(10)
                        .map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join(" ");
                    let is_kf = (vp8_data[0] & 0x01) == 0;
                    if !err.is_null() {
                        let err_str = std::ffi::CStr::from_ptr(err);
                        log::warn!("[SW-VP8Dec] 解码失败({}): {:?}, len={}, kf={}, head=[{}]", 
                            self.decode_error_count, err_str, vp8_data.len(), is_kf, first_bytes);
                    }
                }
                return false;
            }
            
            // 解码成功，重置错误计数
            if self.decode_error_count > 0 {
                log::info!("[SW-VP8Dec] 解码恢复正常，之前失败 {} 次", self.decode_error_count);
                self.decode_error_count = 0;
            }
            
            // 获取解码后的帧
            let mut iter: vpx_codec_iter_t = ptr::null();
            loop {
                let img = vpx_codec_get_frame(&mut self.ctx, &mut iter);
                if img.is_null() {
                    break;
                }
                
                let width = (*img).d_w;
                let height = (*img).d_h;
                
                // 计算各平面大小
                let y_size = (width * height) as usize;
                let u_size = y_size / 4;
                let v_size = u_size;
                let total_size = y_size + u_size + v_size;
                
                // 分配输出缓冲区
                self.dec_output_scratch.resize(total_size, 0u8);
                
                // 复制 Y 平面
                let y_stride = (*img).stride[0] as usize;
                for row in 0..height as usize {
                    ptr::copy_nonoverlapping(
                        (*img).planes[0].add(row * y_stride),
                        self.dec_output_scratch.as_mut_ptr().add(row * width as usize),
                        width as usize,
                    );
                }
                
                // 复制 U 平面
                let u_stride = (*img).stride[1] as usize;
                let uv_width = width as usize / 2;
                let uv_height = height as usize / 2;
                for row in 0..uv_height {
                    ptr::copy_nonoverlapping(
                        (*img).planes[1].add(row * u_stride),
                        self.dec_output_scratch.as_mut_ptr().add(y_size + row * uv_width),
                        uv_width,
                    );
                }
                
                // 复制 V 平面
                let v_stride = (*img).stride[2] as usize;
                for row in 0..uv_height {
                    ptr::copy_nonoverlapping(
                        (*img).planes[2].add(row * v_stride),
                        self.dec_output_scratch.as_mut_ptr().add(y_size + u_size + row * uv_width),
                        uv_width,
                    );
                }
                
                self.output_queue.push_back(DecodedFrame {
                    i420_data: self.dec_output_scratch.clone(),
                    width,
                    height,
                    timestamp_us,
                });

                self.decode_ok_count += 1;
                // 诊断日志: 每 30 帧或前 5 帧输出详细信息
                if self.decode_ok_count <= 5
                    || self.decode_ok_count % 30 == 0
                {
                    let y_xor = self.dec_output_scratch[..y_size]
                        .iter()
                        .fold(0u8, |acc, b| acc ^ b);
                    let u_xor = self.dec_output_scratch[y_size..y_size + u_size]
                        .iter()
                        .fold(0u8, |acc, b| acc ^ b);
                    let v_xor = self.dec_output_scratch[y_size + u_size..y_size + u_size + v_size]
                        .iter()
                        .fold(0u8, |acc, b| acc ^ b);
                    let is_keyframe = (vp8_data[0] & 0x01) == 0;
                    log::info!(
                        "[SW-VP8Dec] frame#{} ok: {}x{}, y_stride={}, u_stride={}, v_stride={}, \
                         data={}B, y_xor={:02x}, u_xor={:02x}, v_xor={:02x}, kf={}",
                        self.decode_ok_count, width, height,
                        y_stride, u_stride, v_stride,
                        vp8_data.len(), y_xor, u_xor, v_xor, is_keyframe,
                    );
                }
                
                log::debug!("[SW-VP8Dec] 解码输出: {}x{}, {} bytes", width, height, total_size);
            }
        }
        
        true
    }
    
    /// 轮询解码输出
    pub fn poll_output(&mut self) -> Option<DecodedFrame> {
        self.output_queue.pop_front()
    }
    
    /// 获取解码器统计
    pub fn get_stats(&self) -> (usize, usize) {
        (self.output_queue.len(), 0)
    }
}

impl Drop for SoftwareVP8Decoder {
    fn drop(&mut self) {
        if self.is_initialized.load(Ordering::SeqCst) {
            unsafe {
                vpx_codec_destroy(&mut self.ctx);
            }
            log::info!("[SW-VP8Dec] 已销毁");
        }
    }
}

// ============================================================
// 线程安全包装器
// ============================================================

/// 线程安全的软件 VP8 编码器
pub struct ThreadSafeSoftwareVP8Encoder {
    inner: Arc<Mutex<SoftwareVP8Encoder>>,
}

impl ThreadSafeSoftwareVP8Encoder {
    pub fn new(config: SoftwareVP8EncoderConfig) -> Self {
        Self {
            inner: Arc::new(Mutex::new(SoftwareVP8Encoder::new(config))),
        }
    }
    
    pub fn initialize(&self) -> bool {
        if let Ok(mut encoder) = self.inner.lock() {
            encoder.initialize()
        } else {
            false
        }
    }
    
    pub fn encode(&self, yuv_data: &[u8], timestamp_us: i64) -> bool {
        if let Ok(mut encoder) = self.inner.lock() {
            encoder.encode(yuv_data, timestamp_us)
        } else {
            false
        }
    }
    
    pub fn encode_nv12(&self, nv12_data: &[u8], timestamp_us: i64) -> bool {
        if let Ok(mut encoder) = self.inner.lock() {
            encoder.encode_nv12(nv12_data, timestamp_us)
        } else {
            false
        }
    }
    
    pub fn poll_output(&self) -> Option<SoftwareEncodedFrame> {
        if let Ok(mut encoder) = self.inner.lock() {
            encoder.poll_output()
        } else {
            None
        }
    }
    
    pub fn request_keyframe(&self) -> bool {
        if let Ok(mut encoder) = self.inner.lock() {
            encoder.request_keyframe()
        } else {
            false
        }
    }
}

/// 线程安全的软件 VP8 解码器
pub struct ThreadSafeSoftwareVP8Decoder {
    inner: Arc<Mutex<SoftwareVP8Decoder>>,
}

impl ThreadSafeSoftwareVP8Decoder {
    pub fn new(config: SoftwareVP8DecoderConfig) -> Self {
        Self {
            inner: Arc::new(Mutex::new(SoftwareVP8Decoder::new(config))),
        }
    }
    
    pub fn initialize(&self) -> bool {
        if let Ok(mut decoder) = self.inner.lock() {
            decoder.initialize()
        } else {
            false
        }
    }
    
    pub fn decode(&self, vp8_data: &[u8], timestamp_us: i64) -> bool {
        if let Ok(mut decoder) = self.inner.lock() {
            decoder.decode(vp8_data, timestamp_us)
        } else {
            false
        }
    }
    
    pub fn poll_output(&self) -> Option<DecodedFrame> {
        if let Ok(mut decoder) = self.inner.lock() {
            decoder.poll_output()
        } else {
            None
        }
    }
}
