//! 图像处理工具
//!
//! 高性能的像素格式转换、stride 去填充等操作。
//! 利用 Rust 编译器自动向量化 (ARM NEON) 实现远超 ArkTS 的处理速度。
//!
//! 该模块从 OHOS 参考实现移植到 LiveKit rust-sdks，纯 Rust 实现，无 unsafe，
//! 无平台相关 FFI，因此对所有 target 均可编译，无需 `cfg(target_env = "ohos")`
//! 守护。

/// 处理摄像头帧数据：去除 stride 对齐填充 + NV21 → NV12 转换
///
/// 摄像头输出的数据格式为 NV21 (YUV_420_SP)：
/// - Y 平面: `width * height` 字节
/// - VU 交织平面: `width * height / 2` 字节（VUVUVU...）
///
/// 编码器需要 NV12 格式：
/// - Y 平面: `width * height` 字节
/// - UV 交织平面: `width * height / 2` 字节（UVUVUV...）
///
/// 当 `stride != width` 时，每行末尾有 `(stride - width)` 字节的填充数据需去除。
///
/// # 性能特点
/// - `chunks_exact_mut(2).swap(0, 1)` 编译为 ARM NEON `vrev16` 指令
/// - `copy_from_slice` 编译为 `memcpy`，走硬件级块拷贝
/// - Release 模式无 bounds check 开销
///
/// # Arguments
/// - `src`: 摄像头原始 NV21 数据
/// - `width`: 图像宽度（像素）
/// - `height`: 图像高度（像素）
/// - `stride`: 行跨距（字节），可能 `>= width`
///
/// # Returns
/// NV12 格式数据（已去除 stride 填充）。如果输入参数非法，返回空 `Vec`。
pub fn process_camera_frame(src: &[u8], width: u32, height: u32, stride: u32) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let s = stride as usize;
    let y_size = w * h;
    let uv_size = w * (h / 2);
    let yuv_size = y_size + uv_size;

    // 防御性检查：宽高为 0 或 stride 小于 width 都属非法输入
    if w == 0 || h == 0 || s < w {
        return Vec::new();
    }

    let mut dst = vec![0u8; yuv_size];

    if s == w {
        // 快速路径：stride == width，整块拷贝 Y + UV
        let copy_len = yuv_size.min(src.len());
        dst[..copy_len].copy_from_slice(&src[..copy_len]);
    } else {
        // stride != width：逐行拷贝以去除每行末尾的填充字节

        // Y 平面
        for row in 0..h {
            let src_off = row * s;
            let dst_off = row * w;
            if src_off + w <= src.len() {
                dst[dst_off..dst_off + w].copy_from_slice(&src[src_off..src_off + w]);
            }
        }

        // UV 平面（NV21 格式，VU 交织）
        let uv_height = h / 2;
        for row in 0..uv_height {
            let src_off = (h + row) * s;
            let dst_off = y_size + row * w;
            if src_off + w <= src.len() {
                dst[dst_off..dst_off + w].copy_from_slice(&src[src_off..src_off + w]);
            }
        }
    }

    // NV21 (VUVU) → NV12 (UVUV)：交换每对字节
    // 此循环会被 LLVM 自动向量化为 ARM NEON vrev16 指令，
    // 一条指令处理 16 字节，远快于 ArkTS 的逐字节循环。
    let uv_plane = &mut dst[y_size..];
    for pair in uv_plane.chunks_exact_mut(2) {
        pair.swap(0, 1);
    }

    dst
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_inputs_return_empty() {
        assert!(process_camera_frame(&[], 0, 0, 0).is_empty());
        assert!(process_camera_frame(&[0u8; 16], 4, 4, 2).is_empty()); // stride < width
    }

    #[test]
    fn fast_path_stride_equals_width_swaps_uv() {
        // 2x2 NV21:  Y=[1,2,3,4]  VU=[10,11]  -> NV12 UV=[11,10]
        let src = [1u8, 2, 3, 4, 10, 11];
        let out = process_camera_frame(&src, 2, 2, 2);
        assert_eq!(out, vec![1, 2, 3, 4, 11, 10]);
    }

    #[test]
    fn slow_path_strips_padding_and_swaps_uv() {
        // 2x2 image with stride=4 (2 padding bytes per row)
        // Y rows: [1,2,_,_], [3,4,_,_]
        // VU row: [10,11,_,_]
        let mut src = vec![0u8; 4 * 2 + 4 * 1];
        src[0] = 1;
        src[1] = 2;
        src[4] = 3;
        src[5] = 4;
        src[8] = 10;
        src[9] = 11;

        let out = process_camera_frame(&src, 2, 2, 4);
        assert_eq!(out, vec![1, 2, 3, 4, 11, 10]);
    }
}
