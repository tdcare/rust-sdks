//! libvpx VP8 软件 编解码器 FFI 绑定
//!
//! 提供纯软件 VP8 编码/解码能力，用于无硬件编解码器的设备
//!
//! ## 关键 API
//! - 编码: vpx_codec_enc_init, vpx_codec_encode, vpx_codec_get_cx_data
//! - 解码: vpx_codec_dec_init, vpx_codec_decode, vpx_codec_get_frame
//!
//! ## 数据流
//! 编码: I420帧 -> vpx_codec_encode -> vpx_codec_get_cx_data -> VP8 比特流
//! 解码: VP8 比特流 -> vpx_codec_decode -> vpx_codec_get_frame -> I420帧

use std::ffi::{c_void, c_int, c_uint, c_ulong, c_char, c_long};
use std::ptr;

// ============================================================
// 错误码
// ============================================================

pub const VPX_CODEC_OK: c_int = 0;
pub const VPX_CODEC_ERROR: c_int = 1;
pub const VPX_CODEC_MEM_ERROR: c_int = 2;
pub const VPX_CODEC_ABI_MISMATCH: c_int = 3;
pub const VPX_CODEC_INCAPABLE: c_int = 4;
pub const VPX_CODEC_UNSUP_BITSTREAM: c_int = 5;
pub const VPX_CODEC_UNSUP_FEATURE: c_int = 6;
pub const VPX_CODEC_CORRUPT_FRAME: c_int = 7;
pub const VPX_CODEC_INVALID_PARAM: c_int = 8;
pub const VPX_CODEC_LIST_END: c_int = 9;

// ============================================================
// 帧标志
// ============================================================

pub const VPX_FRAME_IS_KEY: u32 = 0x1;
pub const VPX_FRAME_IS_DROPPABLE: u32 = 0x2;
pub const VPX_FRAME_IS_INVISIBLE: u32 = 0x4;
pub const VPX_FRAME_IS_FRAGMENT: u32 = 0x8;

// ============================================================
// 编码标志
// ============================================================

pub const VPX_EFLAG_FORCE_KF: c_long = 1 << 0;

// ============================================================
// Deadline 参数
// ============================================================

pub const VPX_DL_REALTIME: c_ulong = 1;
pub const VPX_DL_GOOD_QUALITY: c_ulong = 1000000;
pub const VPX_DL_BEST_QUALITY: c_ulong = 0;

// ============================================================
// 图像格式
// ============================================================

pub const VPX_IMG_FMT_PLANAR: c_int = 0x100;
pub const VPX_IMG_FMT_UV_FLIP: c_int = 0x200;
pub const VPX_IMG_FMT_I420: c_int = VPX_IMG_FMT_PLANAR | 2;
pub const VPX_IMG_FMT_NV12: c_int = VPX_IMG_FMT_PLANAR | 9;
pub const VPX_IMG_FMT_YV12: c_int = VPX_IMG_FMT_PLANAR | VPX_IMG_FMT_UV_FLIP | 1;

// ============================================================
// 码率控制模式
// ============================================================

pub const VPX_VBR: c_int = 0;  // Variable Bit Rate
pub const VPX_CBR: c_int = 1;  // Constant Bit Rate
pub const VPX_CQ: c_int = 2;   // Constrained Quality
pub const VPX_Q: c_int = 3;    // Constant Quality

// ============================================================
// 关键帧模式
// ============================================================

pub const VPX_KF_AUTO: c_int = 1;

// ============================================================
// FFI 结构体
// ============================================================

/// 编解码器接口（不透明）
#[repr(C)]
pub struct vpx_codec_iface {
    _private: [u8; 0],
}

/// 编解码器私有数据（不透明）
#[repr(C)]
pub struct vpx_codec_priv {
    _private: [u8; 0],
}

/// 迭代器
pub type vpx_codec_iter_t = *const c_void;

/// 编解码器上下文
#[repr(C)]
pub struct vpx_codec_ctx_t {
    pub name: *const c_char,
    pub iface: *mut vpx_codec_iface,
    pub err: c_int,
    pub err_detail: *const c_char,
    pub init_flags: c_long,
    pub config: vpx_codec_ctx_config,
    pub priv_: *mut vpx_codec_priv,
}

/// 编解码器配置联合体
#[repr(C)]
pub union vpx_codec_ctx_config {
    pub dec: *const vpx_codec_dec_cfg_t,
    pub enc: *const vpx_codec_enc_cfg_t,
    pub raw: *const c_void,
}

/// 有理数
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct vpx_rational_t {
    pub num: c_int,
    pub den: c_int,
}

/// 编码器配置
#[repr(C)]
pub struct vpx_codec_enc_cfg_t {
    pub g_usage: c_uint,
    pub g_threads: c_uint,
    pub g_profile: c_uint,
    pub g_w: c_uint,
    pub g_h: c_uint,
    pub g_bit_depth: c_int,  // vpx_bit_depth_t
    pub g_input_bit_depth: c_uint,
    pub g_timebase: vpx_rational_t,
    pub g_error_resilient: c_uint,
    pub g_pass: c_int,  // vpx_enc_pass
    pub g_lag_in_frames: c_uint,
    
    // Rate control
    pub rc_dropframe_thresh: c_uint,
    pub rc_resize_allowed: c_uint,
    pub rc_scaled_width: c_uint,
    pub rc_scaled_height: c_uint,
    pub rc_resize_up_thresh: c_uint,
    pub rc_resize_down_thresh: c_uint,
    pub rc_end_usage: c_int,  // vpx_rc_mode
    pub rc_twopass_stats_in: vpx_fixed_buf_t,
    pub rc_firstpass_mb_stats_in: vpx_fixed_buf_t,
    pub rc_target_bitrate: c_uint,
    pub rc_min_quantizer: c_uint,
    pub rc_max_quantizer: c_uint,
    pub rc_undershoot_pct: c_uint,
    pub rc_overshoot_pct: c_uint,
    pub rc_buf_sz: c_uint,
    pub rc_buf_initial_sz: c_uint,
    pub rc_buf_optimal_sz: c_uint,
    pub rc_2pass_vbr_bias_pct: c_uint,
    pub rc_2pass_vbr_minsection_pct: c_uint,
    pub rc_2pass_vbr_maxsection_pct: c_uint,
    pub rc_2pass_vbr_corpus_complexity: c_uint,
    
    // Keyframe settings
    pub kf_mode: c_int,  // vpx_kf_mode
    pub kf_min_dist: c_uint,
    pub kf_max_dist: c_uint,
    
    // Spatial scalability (简化版，不使用 SVC)
    pub ss_number_layers: c_uint,
    pub ss_enable_auto_alt_ref: [c_int; 5],
    pub ss_target_bitrate: [c_uint; 5],
    pub ts_number_layers: c_uint,
    pub ts_target_bitrate: [c_uint; 5],
    pub ts_rate_decimator: [c_uint; 5],
    pub ts_periodicity: c_uint,
    pub ts_layer_id: [c_uint; 16],
    pub layer_target_bitrate: [c_uint; 12],
    pub temporal_layering_mode: c_int,
    pub use_vizier_rc_params: c_int,
    
    // Rate control parameters (简化)
    pub active_wq_factor: vpx_rational_t,
    pub err_per_mb_factor: vpx_rational_t,
    pub sr_default_decay_limit: vpx_rational_t,
    pub sr_diff_factor: vpx_rational_t,
    pub kf_err_per_mb_factor: vpx_rational_t,
    pub kf_frame_min_boost_factor: vpx_rational_t,
    pub kf_frame_max_boost_first_factor: vpx_rational_t,
    pub kf_frame_max_boost_subs_factor: vpx_rational_t,
    pub kf_max_total_boost_factor: vpx_rational_t,
    pub gf_max_total_boost_factor: vpx_rational_t,
    pub gf_frame_max_boost_factor: vpx_rational_t,
    pub zm_factor: vpx_rational_t,
    pub rd_mult_inter_qp_fac: vpx_rational_t,
    pub rd_mult_arf_qp_fac: vpx_rational_t,
    pub rd_mult_key_qp_fac: vpx_rational_t,
}

/// 解码器配置
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct vpx_codec_dec_cfg_t {
    pub threads: c_uint,
    pub w: c_uint,
    pub h: c_uint,
}

/// 固定缓冲区
#[repr(C)]
#[derive(Clone, Copy)]
pub struct vpx_fixed_buf_t {
    pub buf: *mut c_void,
    pub sz: usize,
}

/// 时间戳类型
pub type vpx_codec_pts_t = i64;

/// 编码帧标志
pub type vpx_codec_frame_flags_t = u32;

/// 编码帧标志类型
pub type vpx_enc_frame_flags_t = c_long;

/// 输出包类型
#[repr(C)]
pub enum vpx_codec_cx_pkt_kind {
    VPX_CODEC_CX_FRAME_PKT = 0,
    VPX_CODEC_STATS_PKT = 1,
    VPX_CODEC_FPMB_STATS_PKT = 2,
    VPX_CODEC_PSNR_PKT = 3,
    VPX_CODEC_CUSTOM_PKT = 256,
}

/// 编码帧数据
#[repr(C)]
#[derive(Clone, Copy)]
pub struct vpx_codec_cx_frame {
    pub buf: *mut c_void,
    pub sz: usize,
    pub pts: vpx_codec_pts_t,
    pub duration: c_ulong,
    pub flags: vpx_codec_frame_flags_t,
    pub partition_id: c_int,
    pub width: [c_uint; 5],
    pub height: [c_uint; 5],
    pub spatial_layer_encoded: [u8; 5],
}

/// 输出包 - 匹配 C 结构体 vpx_codec_cx_pkt
/// 
/// C 布局 (aarch64):
///   kind: c_int (4 bytes) at offset 0
///   [4 bytes padding] — union 需要 8 字节对齐（含指针）
///   data: union (128 bytes) at offset 8
///   Total: 136 bytes
///
/// 我们只关心 VPX_CODEC_CX_FRAME_PKT 类型，直接使用 frame 结构体
/// 替代字节数组，让 #[repr(C)] 自动处理对齐 padding
#[repr(C)]
pub struct vpx_codec_cx_pkt_t {
    pub kind: c_int,  // vpx_codec_cx_pkt_kind (4 bytes)
    // #[repr(C)] 自动在此插入 4 bytes padding，因为 frame 的对齐要求为 8（含指针）
    pub frame: vpx_codec_cx_frame,  // 直接访问 frame variant
}

/// 图像描述符
#[repr(C)]
pub struct vpx_image_t {
    pub fmt: c_int,           // vpx_img_fmt_t
    pub cs: c_int,            // vpx_color_space_t
    pub range: c_int,         // vpx_color_range_t
    pub w: c_uint,            // 存储宽度
    pub h: c_uint,            // 存储高度
    pub bit_depth: c_uint,    // 位深度
    pub d_w: c_uint,          // 显示宽度
    pub d_h: c_uint,          // 显示高度
    pub r_w: c_uint,          // 渲染宽度
    pub r_h: c_uint,          // 渲染高度
    pub x_chroma_shift: c_uint,
    pub y_chroma_shift: c_uint,
    pub planes: [*mut u8; 4],  // Y, U, V, Alpha
    pub stride: [c_int; 4],
    pub bps: c_int,
    pub user_priv: *mut c_void,
    pub img_data: *mut u8,
    pub img_data_owner: c_int,
    pub self_allocd: c_int,
    pub fb_priv: *mut c_void,
}

/// 流信息
#[repr(C)]
pub struct vpx_codec_stream_info_t {
    pub sz: c_uint,
    pub w: c_uint,
    pub h: c_uint,
    pub is_kf: c_uint,
}

// ============================================================
// FFI 函数声明
// ============================================================

// VP8 编码器接口
extern "C" {
    /// 获取 VP8 编码器接口
    pub fn vpx_codec_vp8_cx() -> *mut vpx_codec_iface;
    
    /// 获取 VP9 编码器接口
    pub fn vpx_codec_vp9_cx() -> *mut vpx_codec_iface;
    
    /// 获取 VP8 解码器接口
    pub fn vpx_codec_vp8_dx() -> *mut vpx_codec_iface;
    
    /// 获取 VP9 解码器接口
    pub fn vpx_codec_vp9_dx() -> *mut vpx_codec_iface;
}

// 编解码器通用
extern "C" {
    /// 销毁编解码器实例
    pub fn vpx_codec_destroy(ctx: *mut vpx_codec_ctx_t) -> c_int;
    
    /// 获取错误字符串
    pub fn vpx_codec_error(ctx: *const vpx_codec_ctx_t) -> *const c_char;
    
    /// 获取详细错误信息
    pub fn vpx_codec_error_detail(ctx: *const vpx_codec_ctx_t) -> *const c_char;
    
    /// 获取版本号
    pub fn vpx_codec_version() -> c_int;
    
    /// 获取版本字符串
    pub fn vpx_codec_version_str() -> *const c_char;
}

// 编码器 API
extern "C" {
    /// 初始化编码器
    pub fn vpx_codec_enc_init_ver(
        ctx: *mut vpx_codec_ctx_t,
        iface: *mut vpx_codec_iface,
        cfg: *const vpx_codec_enc_cfg_t,
        flags: c_long,
        ver: c_int,
    ) -> c_int;
    
    /// 获取默认编码器配置
    pub fn vpx_codec_enc_config_default(
        iface: *mut vpx_codec_iface,
        cfg: *mut vpx_codec_enc_cfg_t,
        usage: c_uint,
    ) -> c_int;
    
    /// 设置编码器配置
    pub fn vpx_codec_enc_config_set(
        ctx: *mut vpx_codec_ctx_t,
        cfg: *const vpx_codec_enc_cfg_t,
    ) -> c_int;
    
    /// 编码一帧
    pub fn vpx_codec_encode(
        ctx: *mut vpx_codec_ctx_t,
        img: *const vpx_image_t,
        pts: vpx_codec_pts_t,
        duration: c_ulong,
        flags: vpx_enc_frame_flags_t,
        deadline: c_ulong,
    ) -> c_int;
    
    /// 获取编码输出数据
    pub fn vpx_codec_get_cx_data(
        ctx: *mut vpx_codec_ctx_t,
        iter: *mut vpx_codec_iter_t,
    ) -> *const vpx_codec_cx_pkt_t;
    
    /// 控制命令（用于强制关键帧等）
    pub fn vpx_codec_control_(
        ctx: *mut vpx_codec_ctx_t,
        ctrl_id: c_int,
        ...
    ) -> c_int;
}

// 解码器 API
extern "C" {
    /// 初始化解码器
    pub fn vpx_codec_dec_init_ver(
        ctx: *mut vpx_codec_ctx_t,
        iface: *mut vpx_codec_iface,
        cfg: *const vpx_codec_dec_cfg_t,
        flags: c_long,
        ver: c_int,
    ) -> c_int;
    
    /// 解码数据
    pub fn vpx_codec_decode(
        ctx: *mut vpx_codec_ctx_t,
        data: *const u8,
        data_sz: c_uint,
        user_priv: *mut c_void,
        deadline: c_long,
    ) -> c_int;
    
    /// 获取解码后的帧
    pub fn vpx_codec_get_frame(
        ctx: *mut vpx_codec_ctx_t,
        iter: *mut vpx_codec_iter_t,
    ) -> *mut vpx_image_t;
    
    /// 获取流信息
    pub fn vpx_codec_get_stream_info(
        ctx: *mut vpx_codec_ctx_t,
        si: *mut vpx_codec_stream_info_t,
    ) -> c_int;
}

// 图像 API
extern "C" {
    /// 分配图像
    pub fn vpx_img_alloc(
        img: *mut vpx_image_t,
        fmt: c_int,
        d_w: c_uint,
        d_h: c_uint,
        align: c_uint,
    ) -> *mut vpx_image_t;
    
    /// 使用现有内存包装图像
    pub fn vpx_img_wrap(
        img: *mut vpx_image_t,
        fmt: c_int,
        d_w: c_uint,
        d_h: c_uint,
        stride_align: c_uint,
        img_data: *mut u8,
    ) -> *mut vpx_image_t;
    
    /// 释放图像
    pub fn vpx_img_free(img: *mut vpx_image_t);
    
    /// 设置图像区域
    pub fn vpx_img_set_rect(
        img: *mut vpx_image_t,
        x: c_uint,
        y: c_uint,
        w: c_uint,
        h: c_uint,
    ) -> c_int;
}

// ============================================================
// 辅助常量和 ABI 版本
// ============================================================

// VPX_IMAGE_ABI_VERSION = 5
// VPX_CODEC_ABI_VERSION = 4 + VPX_IMAGE_ABI_VERSION = 9
// VPX_EXT_RATECTRL_ABI_VERSION = 7
// VPX_TPL_ABI_VERSION = 2
pub const VPX_ENCODER_ABI_VERSION: c_int = 16 + 9 + 7 + 2;  // = 34
pub const VPX_DECODER_ABI_VERSION: c_int = 3 + 9;  // = 12

// 控制命令 ID
pub const VP8E_SET_CPUUSED: c_int = 13;
pub const VP8E_SET_ENABLEAUTOALTREF: c_int = 14;
pub const VP8E_SET_NOISE_SENSITIVITY: c_int = 15;
pub const VP8E_SET_SHARPNESS: c_int = 16;
pub const VP8E_SET_STATIC_THRESHOLD: c_int = 17;
pub const VP8E_GET_LAST_QUANTIZER: c_int = 20;
pub const VP8E_GET_LAST_QUANTIZER_64: c_int = 21;
pub const VP8E_SET_ARNR_MAXFRAMES: c_int = 23;
pub const VP8E_SET_ARNR_STRENGTH: c_int = 24;
pub const VP8E_SET_CQ_LEVEL: c_int = 30;
pub const VP8E_SET_MAX_INTRA_BITRATE_PCT: c_int = 31;

// ============================================================
// 辅助函数
// ============================================================

/// 检查编码器是否初始化成功
pub unsafe fn vpx_codec_enc_init(
    ctx: *mut vpx_codec_ctx_t,
    iface: *mut vpx_codec_iface,
    cfg: *const vpx_codec_enc_cfg_t,
    flags: c_long,
) -> c_int {
    vpx_codec_enc_init_ver(ctx, iface, cfg, flags, VPX_ENCODER_ABI_VERSION)
}

/// 检查解码器是否初始化成功
pub unsafe fn vpx_codec_dec_init(
    ctx: *mut vpx_codec_ctx_t,
    iface: *mut vpx_codec_iface,
    cfg: *const vpx_codec_dec_cfg_t,
    flags: c_long,
) -> c_int {
    vpx_codec_dec_init_ver(ctx, iface, cfg, flags, VPX_DECODER_ABI_VERSION)
}

/// 从编码包中提取帧数据
/// 返回 (数据指针, 数据大小, 是否关键帧)
pub unsafe fn extract_frame_from_pkt(pkt: *const vpx_codec_cx_pkt_t) -> Option<(*const u8, usize, bool)> {
    if pkt.is_null() {
        return None;
    }
    
    let kind = (*pkt).kind;
    if kind != vpx_codec_cx_pkt_kind::VPX_CODEC_CX_FRAME_PKT as c_int {
        return None;
    }
    
    // 直接通过结构体字段访问，不再手动解析字节偏移
    let frame = &(*pkt).frame;
    let buf = frame.buf as *const u8;
    let sz = frame.sz;
    let flags = frame.flags;
    
    if buf.is_null() || sz == 0 {
        return None;
    }
    
    let is_key_frame = (flags & VPX_FRAME_IS_KEY) != 0;
    Some((buf, sz, is_key_frame))
}
