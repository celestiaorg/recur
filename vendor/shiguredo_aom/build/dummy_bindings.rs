// DOCS_RS ビルド専用のダミー bindings。
//
// docs.rs では git clone ができないため、libaom をビルドして bindings を
// 生成することができない。そこで、src/lib.rs が参照するシンボルとその依存
// 定義を、source-build で生成した bindings.rs から抽出してここに置いている。
//
// このファイルは機械的な抽出物であり、手動で定義を追加・変更しないこと。
// libaom を更新する際は、以下の手順で再抽出すること。
//   1. `cargo build --features source-build` で bindings.rs を生成する
//   2. 生成された bindings.rs から、src/lib.rs が参照するシンボルとその依存
//      定義 (struct / union / type / const / extern 関数) を抽出する
//   3. `rustfmt` で整形する
// 定数値は libaom v3.15.1 の実値を反映している。
//
// -----------------------------------------------------------------------------
// 型エイリアス
// -----------------------------------------------------------------------------
pub type aom_codec_ctx_t = aom_codec_ctx;
pub type aom_codec_cx_pkt_t = aom_codec_cx_pkt;
pub type aom_codec_dec_cfg_t = aom_codec_dec_cfg;
pub type aom_codec_enc_cfg_t = aom_codec_enc_cfg;
pub type aom_codec_er_flags_t = u32;
pub type aom_codec_err_t = ::std::os::raw::c_uint;
pub type aom_codec_flags_t = ::std::os::raw::c_long;
pub type aom_codec_frame_flags_t = u32;
pub type aom_codec_iter_t = *const ::std::os::raw::c_void;
pub type aom_codec_priv_t = aom_codec_priv;
pub type aom_codec_pts_t = i64;
pub type aom_enc_frame_flags_t = ::std::os::raw::c_long;
pub type aom_enc_pass = ::std::os::raw::c_uint;
pub type aom_fixed_buf_t = aom_fixed_buf;
pub type aom_image_t = aom_image;
pub type aom_kf_mode = ::std::os::raw::c_uint;
pub type aom_metadata_array_t = aom_metadata_array;
pub type aom_rc_mode = ::std::os::raw::c_uint;
pub type aom_superres_mode = ::std::os::raw::c_uint;
pub type aom_img_fmt = ::std::os::raw::c_uint;
pub use self::aom_img_fmt as aom_img_fmt_t;
pub type aom_color_primaries = ::std::os::raw::c_uint;
pub use self::aom_color_primaries as aom_color_primaries_t;
pub type aom_transfer_characteristics = ::std::os::raw::c_uint;
pub use self::aom_transfer_characteristics as aom_transfer_characteristics_t;
pub type aom_matrix_coefficients = ::std::os::raw::c_uint;
pub use self::aom_matrix_coefficients as aom_matrix_coefficients_t;
pub type aom_color_range = ::std::os::raw::c_uint;
pub use self::aom_color_range as aom_color_range_t;
pub type aom_chroma_sample_position = ::std::os::raw::c_uint;
pub use self::aom_chroma_sample_position as aom_chroma_sample_position_t;
pub type aom_bit_depth = ::std::os::raw::c_uint;
pub use self::aom_bit_depth as aom_bit_depth_t;
pub type aom_superblock_size = ::std::os::raw::c_uint;
pub use self::aom_superblock_size as aom_superblock_size_t;
pub type aom_codec_cx_pkt_kind = ::std::os::raw::c_uint;
pub type aom_tune_content = ::std::os::raw::c_uint;
pub type aome_enc_control_id = ::std::os::raw::c_uint;
pub type cfg_options_t = cfg_options;

// -----------------------------------------------------------------------------
// struct / union
// -----------------------------------------------------------------------------
#[repr(C)]
#[derive(Copy, Clone)]
pub struct aom_codec_ctx {
    pub name: *const ::std::os::raw::c_char,
    pub iface: *const aom_codec_iface,
    pub err: aom_codec_err_t,
    pub err_detail: *const ::std::os::raw::c_char,
    pub init_flags: aom_codec_flags_t,
    pub config: aom_codec_ctx__bindgen_ty_1,
    pub priv_: *mut aom_codec_priv_t,
}
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct aom_codec_cx_pkt__bindgen_ty_1__bindgen_ty_1 {
    pub buf: *mut ::std::os::raw::c_void,
    pub sz: usize,
    pub pts: aom_codec_pts_t,
    pub duration: ::std::os::raw::c_ulong,
    pub flags: aom_codec_frame_flags_t,
    pub partition_id: ::std::os::raw::c_int,
    pub vis_frame_size: usize,
}
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct aom_codec_dec_cfg {
    pub threads: ::std::os::raw::c_uint,
    pub w: ::std::os::raw::c_uint,
    pub h: ::std::os::raw::c_uint,
    pub allow_lowbitdepth: ::std::os::raw::c_uint,
}
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct aom_codec_enc_cfg {
    pub g_usage: ::std::os::raw::c_uint,
    pub g_threads: ::std::os::raw::c_uint,
    pub g_profile: ::std::os::raw::c_uint,
    pub g_w: ::std::os::raw::c_uint,
    pub g_h: ::std::os::raw::c_uint,
    pub g_limit: ::std::os::raw::c_uint,
    pub g_forced_max_frame_width: ::std::os::raw::c_uint,
    pub g_forced_max_frame_height: ::std::os::raw::c_uint,
    pub g_bit_depth: aom_bit_depth_t,
    pub g_input_bit_depth: ::std::os::raw::c_uint,
    pub g_timebase: aom_rational,
    pub g_error_resilient: aom_codec_er_flags_t,
    pub g_pass: aom_enc_pass,
    pub g_lag_in_frames: ::std::os::raw::c_uint,
    pub rc_dropframe_thresh: ::std::os::raw::c_uint,
    pub rc_resize_mode: ::std::os::raw::c_uint,
    pub rc_resize_denominator: ::std::os::raw::c_uint,
    pub rc_resize_kf_denominator: ::std::os::raw::c_uint,
    pub rc_superres_mode: aom_superres_mode,
    pub rc_superres_denominator: ::std::os::raw::c_uint,
    pub rc_superres_kf_denominator: ::std::os::raw::c_uint,
    pub rc_superres_qthresh: ::std::os::raw::c_uint,
    pub rc_superres_kf_qthresh: ::std::os::raw::c_uint,
    pub rc_end_usage: aom_rc_mode,
    pub rc_twopass_stats_in: aom_fixed_buf_t,
    pub rc_firstpass_mb_stats_in: aom_fixed_buf_t,
    pub rc_target_bitrate: ::std::os::raw::c_uint,
    pub rc_min_quantizer: ::std::os::raw::c_uint,
    pub rc_max_quantizer: ::std::os::raw::c_uint,
    pub rc_undershoot_pct: ::std::os::raw::c_uint,
    pub rc_overshoot_pct: ::std::os::raw::c_uint,
    pub rc_buf_sz: ::std::os::raw::c_uint,
    pub rc_buf_initial_sz: ::std::os::raw::c_uint,
    pub rc_buf_optimal_sz: ::std::os::raw::c_uint,
    pub rc_2pass_vbr_bias_pct: ::std::os::raw::c_uint,
    pub rc_2pass_vbr_minsection_pct: ::std::os::raw::c_uint,
    pub rc_2pass_vbr_maxsection_pct: ::std::os::raw::c_uint,
    pub fwd_kf_enabled: ::std::os::raw::c_int,
    pub kf_mode: aom_kf_mode,
    pub kf_min_dist: ::std::os::raw::c_uint,
    pub kf_max_dist: ::std::os::raw::c_uint,
    pub sframe_dist: ::std::os::raw::c_uint,
    pub sframe_mode: ::std::os::raw::c_uint,
    pub large_scale_tile: ::std::os::raw::c_uint,
    pub monochrome: ::std::os::raw::c_uint,
    pub full_still_picture_hdr: ::std::os::raw::c_uint,
    pub save_as_annexb: ::std::os::raw::c_uint,
    pub tile_width_count: ::std::os::raw::c_int,
    pub tile_height_count: ::std::os::raw::c_int,
    pub tile_widths: [::std::os::raw::c_int; 64usize],
    pub tile_heights: [::std::os::raw::c_int; 64usize],
    pub use_fixed_qp_offsets: ::std::os::raw::c_uint,
    pub fixed_qp_offsets: [::std::os::raw::c_int; 5usize],
    pub encoder_cfg: cfg_options_t,
}
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct aom_codec_iface {
    _unused: [u8; 0],
}
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct aom_image {
    pub fmt: aom_img_fmt_t,
    pub cp: aom_color_primaries_t,
    pub tc: aom_transfer_characteristics_t,
    pub mc: aom_matrix_coefficients_t,
    pub monochrome: ::std::os::raw::c_int,
    pub csp: aom_chroma_sample_position_t,
    pub range: aom_color_range_t,
    pub w: ::std::os::raw::c_uint,
    pub h: ::std::os::raw::c_uint,
    pub bit_depth: ::std::os::raw::c_uint,
    pub d_w: ::std::os::raw::c_uint,
    pub d_h: ::std::os::raw::c_uint,
    pub r_w: ::std::os::raw::c_uint,
    pub r_h: ::std::os::raw::c_uint,
    pub x_chroma_shift: ::std::os::raw::c_uint,
    pub y_chroma_shift: ::std::os::raw::c_uint,
    pub planes: [*mut ::std::os::raw::c_uchar; 3usize],
    pub stride: [::std::os::raw::c_int; 3usize],
    pub sz: usize,
    pub bps: ::std::os::raw::c_int,
    pub temporal_id: ::std::os::raw::c_int,
    pub spatial_id: ::std::os::raw::c_int,
    pub user_priv: *mut ::std::os::raw::c_void,
    pub img_data: *mut ::std::os::raw::c_uchar,
    pub img_data_owner: ::std::os::raw::c_int,
    pub self_allocd: ::std::os::raw::c_int,
    pub metadata: *mut aom_metadata_array_t,
    pub fb_priv: *mut ::std::os::raw::c_void,
}
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct aom_rational {
    pub num: ::std::os::raw::c_int,
    pub den: ::std::os::raw::c_int,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub union aom_codec_ctx__bindgen_ty_1 {
    pub dec: *const aom_codec_dec_cfg,
    pub enc: *const aom_codec_enc_cfg,
    pub raw: *const ::std::os::raw::c_void,
}
#[repr(C)]
#[derive(Copy, Clone)]
pub struct aom_codec_cx_pkt {
    pub kind: aom_codec_cx_pkt_kind,
    pub data: aom_codec_cx_pkt__bindgen_ty_1,
}
#[repr(C)]
#[derive(Copy, Clone)]
pub union aom_codec_cx_pkt__bindgen_ty_1 {
    pub frame: aom_codec_cx_pkt__bindgen_ty_1__bindgen_ty_1,
    pub twopass_stats: aom_fixed_buf_t,
    pub firstpass_mb_stats: aom_fixed_buf_t,
    pub psnr: aom_codec_cx_pkt__bindgen_ty_1_aom_psnr_pkt,
    pub raw: aom_fixed_buf_t,
}
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct aom_codec_cx_pkt__bindgen_ty_1_aom_psnr_pkt {
    pub samples: [::std::os::raw::c_uint; 4usize],
    pub sse: [u64; 4usize],
    pub psnr: [f64; 4usize],
    pub samples_hbd: [::std::os::raw::c_uint; 4usize],
    pub sse_hbd: [u64; 4usize],
    pub psnr_hbd: [f64; 4usize],
}
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct aom_codec_priv {
    _unused: [u8; 0],
}
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct aom_fixed_buf {
    pub buf: *mut ::std::os::raw::c_void,
    pub sz: usize,
}
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct aom_metadata_array {
    _unused: [u8; 0],
}
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct cfg_options {
    pub init_by_cfg_file: ::std::os::raw::c_uint,
    pub super_block_size: ::std::os::raw::c_uint,
    pub max_partition_size: ::std::os::raw::c_uint,
    pub min_partition_size: ::std::os::raw::c_uint,
    pub disable_ab_partition_type: ::std::os::raw::c_uint,
    pub disable_rect_partition_type: ::std::os::raw::c_uint,
    pub disable_1to4_partition_type: ::std::os::raw::c_uint,
    pub disable_flip_idtx: ::std::os::raw::c_uint,
    pub disable_cdef: ::std::os::raw::c_uint,
    pub disable_lr: ::std::os::raw::c_uint,
    pub disable_obmc: ::std::os::raw::c_uint,
    pub disable_warp_motion: ::std::os::raw::c_uint,
    pub disable_global_motion: ::std::os::raw::c_uint,
    pub disable_dist_wtd_comp: ::std::os::raw::c_uint,
    pub disable_diff_wtd_comp: ::std::os::raw::c_uint,
    pub disable_inter_intra_comp: ::std::os::raw::c_uint,
    pub disable_masked_comp: ::std::os::raw::c_uint,
    pub disable_one_sided_comp: ::std::os::raw::c_uint,
    pub disable_palette: ::std::os::raw::c_uint,
    pub disable_intrabc: ::std::os::raw::c_uint,
    pub disable_cfl: ::std::os::raw::c_uint,
    pub disable_smooth_intra: ::std::os::raw::c_uint,
    pub disable_filter_intra: ::std::os::raw::c_uint,
    pub disable_dual_filter: ::std::os::raw::c_uint,
    pub disable_intra_angle_delta: ::std::os::raw::c_uint,
    pub disable_intra_edge_filter: ::std::os::raw::c_uint,
    pub disable_tx_64x64: ::std::os::raw::c_uint,
    pub disable_smooth_inter_intra: ::std::os::raw::c_uint,
    pub disable_inter_inter_wedge: ::std::os::raw::c_uint,
    pub disable_inter_intra_wedge: ::std::os::raw::c_uint,
    pub disable_paeth_intra: ::std::os::raw::c_uint,
    pub disable_trellis_quant: ::std::os::raw::c_uint,
    pub disable_ref_frame_mv: ::std::os::raw::c_uint,
    pub reduced_reference_set: ::std::os::raw::c_uint,
    pub reduced_tx_type_set: ::std::os::raw::c_uint,
}

// -----------------------------------------------------------------------------
// 定数
// -----------------------------------------------------------------------------
pub const AOM_CODEC_USE_HIGHBITDEPTH: u32 = 262144;
pub const AOM_DECODER_ABI_VERSION: u32 = 22;
pub const AOM_EFLAG_FORCE_KF: u32 = 1;
pub const AOM_ENCODER_ABI_VERSION: u32 = 29;
pub const AOM_FRAME_IS_KEY: u32 = 1;
pub const AOM_USAGE_ALL_INTRA: u32 = 2;
pub const AOM_USAGE_GOOD_QUALITY: u32 = 0;
pub const AOM_USAGE_REALTIME: u32 = 1;
pub const aom_codec_cx_pkt_kind_AOM_CODEC_CX_FRAME_PKT: aom_codec_cx_pkt_kind = 0;
pub const aom_codec_err_t_AOM_CODEC_ERROR: aom_codec_err_t = 1;
pub const aom_codec_err_t_AOM_CODEC_INVALID_PARAM: aom_codec_err_t = 8;
pub const aom_codec_err_t_AOM_CODEC_MEM_ERROR: aom_codec_err_t = 2;
pub const aom_codec_err_t_AOM_CODEC_OK: aom_codec_err_t = 0;
pub const aom_enc_pass_AOM_RC_ONE_PASS: aom_enc_pass = 0;
pub const aom_img_fmt_AOM_IMG_FMT_I42016: aom_img_fmt = 2306;
pub const aom_img_fmt_AOM_IMG_FMT_I420: aom_img_fmt = 258;
pub const aom_img_fmt_AOM_IMG_FMT_I42216: aom_img_fmt = 2309;
pub const aom_img_fmt_AOM_IMG_FMT_I422: aom_img_fmt = 261;
pub const aom_img_fmt_AOM_IMG_FMT_I44416: aom_img_fmt = 2310;
pub const aom_img_fmt_AOM_IMG_FMT_I444: aom_img_fmt = 262;
pub const aom_img_fmt_AOM_IMG_FMT_NV12: aom_img_fmt = 263;
pub const aom_img_fmt_AOM_IMG_FMT_YV12: aom_img_fmt = 769;
pub const aom_kf_mode_AOM_KF_AUTO: aom_kf_mode = 1;
pub const aom_kf_mode_AOM_KF_DISABLED: aom_kf_mode = 0;
pub const aom_kf_mode_AOM_KF_FIXED: aom_kf_mode = 0;
pub const aom_rc_mode_AOM_CBR: aom_rc_mode = 1;
pub const aom_rc_mode_AOM_CQ: aom_rc_mode = 2;
pub const aom_rc_mode_AOM_Q: aom_rc_mode = 3;
pub const aom_rc_mode_AOM_VBR: aom_rc_mode = 0;
pub const aom_superblock_size_AOM_SUPERBLOCK_SIZE_128X128: aom_superblock_size = 1;
pub const aom_superblock_size_AOM_SUPERBLOCK_SIZE_64X64: aom_superblock_size = 0;
pub const aom_superblock_size_AOM_SUPERBLOCK_SIZE_DYNAMIC: aom_superblock_size = 2;
pub const aom_tune_content_AOM_CONTENT_DEFAULT: aom_tune_content = 0;
pub const aom_tune_content_AOM_CONTENT_FILM: aom_tune_content = 2;
pub const aom_tune_content_AOM_CONTENT_SCREEN: aom_tune_content = 1;
pub const aome_enc_control_id_AOME_SET_ARNR_MAXFRAMES: aome_enc_control_id = 21;
pub const aome_enc_control_id_AOME_SET_ARNR_STRENGTH: aome_enc_control_id = 22;
pub const aome_enc_control_id_AOME_SET_CPUUSED: aome_enc_control_id = 13;
pub const aome_enc_control_id_AOME_SET_CQ_LEVEL: aome_enc_control_id = 25;
pub const aome_enc_control_id_AOME_SET_MAX_INTRA_BITRATE_PCT: aome_enc_control_id = 26;
pub const aome_enc_control_id_AOME_SET_SHARPNESS: aome_enc_control_id = 16;
pub const aome_enc_control_id_AOME_SET_STATIC_THRESHOLD: aome_enc_control_id = 17;
pub const aome_enc_control_id_AV1E_SET_AQ_MODE: aome_enc_control_id = 40;
pub const aome_enc_control_id_AV1E_SET_COEFF_COST_UPD_FREQ: aome_enc_control_id = 126;
pub const aome_enc_control_id_AV1E_SET_COLOR_PRIMARIES: aome_enc_control_id = 45;
pub const aome_enc_control_id_AV1E_SET_COLOR_RANGE: aome_enc_control_id = 52;
pub const aome_enc_control_id_AV1E_SET_DELTAQ_MODE: aome_enc_control_id = 107;
pub const aome_enc_control_id_AV1E_SET_DENOISE_BLOCK_SIZE: aome_enc_control_id = 115;
pub const aome_enc_control_id_AV1E_SET_DENOISE_NOISE_LEVEL: aome_enc_control_id = 114;
pub const aome_enc_control_id_AV1E_SET_ENABLE_1TO4_PARTITIONS: aome_enc_control_id = 75;
pub const aome_enc_control_id_AV1E_SET_ENABLE_AB_PARTITIONS: aome_enc_control_id = 74;
pub const aome_enc_control_id_AV1E_SET_ENABLE_ANGLE_DELTA: aome_enc_control_id = 106;
pub const aome_enc_control_id_AV1E_SET_ENABLE_CDEF: aome_enc_control_id = 58;
pub const aome_enc_control_id_AV1E_SET_ENABLE_CFL_INTRA: aome_enc_control_id = 101;
pub const aome_enc_control_id_AV1E_SET_ENABLE_CHROMA_DELTAQ: aome_enc_control_id = 87;
pub const aome_enc_control_id_AV1E_SET_ENABLE_DUAL_FILTER: aome_enc_control_id = 86;
pub const aome_enc_control_id_AV1E_SET_ENABLE_FILTER_INTRA: aome_enc_control_id = 98;
pub const aome_enc_control_id_AV1E_SET_ENABLE_GLOBAL_MOTION: aome_enc_control_id = 95;
pub const aome_enc_control_id_AV1E_SET_ENABLE_INTRABC: aome_enc_control_id = 105;
pub const aome_enc_control_id_AV1E_SET_ENABLE_KEYFRAME_FILTERING: aome_enc_control_id = 36;
pub const aome_enc_control_id_AV1E_SET_ENABLE_OBMC: aome_enc_control_id = 61;
pub const aome_enc_control_id_AV1E_SET_ENABLE_ORDER_HINT: aome_enc_control_id = 79;
pub const aome_enc_control_id_AV1E_SET_ENABLE_PAETH_INTRA: aome_enc_control_id = 100;
pub const aome_enc_control_id_AV1E_SET_ENABLE_PALETTE: aome_enc_control_id = 104;
pub const aome_enc_control_id_AV1E_SET_ENABLE_RECT_PARTITIONS: aome_enc_control_id = 73;
pub const aome_enc_control_id_AV1E_SET_ENABLE_REF_FRAME_MVS: aome_enc_control_id = 84;
pub const aome_enc_control_id_AV1E_SET_ENABLE_RESTORATION: aome_enc_control_id = 59;
pub const aome_enc_control_id_AV1E_SET_ENABLE_SMOOTH_INTRA: aome_enc_control_id = 99;
pub const aome_enc_control_id_AV1E_SET_ENABLE_SUPERRES: aome_enc_control_id = 102;
pub const aome_enc_control_id_AV1E_SET_ENABLE_TPL_MODEL: aome_enc_control_id = 35;
pub const aome_enc_control_id_AV1E_SET_ENABLE_WARPED_MOTION: aome_enc_control_id = 96;
pub const aome_enc_control_id_AV1E_SET_FILM_GRAIN_TEST_VECTOR: aome_enc_control_id = 112;
pub const aome_enc_control_id_AV1E_SET_GF_MAX_PYRAMID_HEIGHT: aome_enc_control_id = 123;
pub const aome_enc_control_id_AV1E_SET_INTRA_DEFAULT_TX_ONLY: aome_enc_control_id = 121;
pub const aome_enc_control_id_AV1E_SET_LOOPFILTER_CONTROL: aome_enc_control_id = 149;
pub const aome_enc_control_id_AV1E_SET_LOSSLESS: aome_enc_control_id = 31;
pub const aome_enc_control_id_AV1E_SET_MATRIX_COEFFICIENTS: aome_enc_control_id = 47;
pub const aome_enc_control_id_AV1E_SET_MAX_GF_INTERVAL: aome_enc_control_id = 50;
pub const aome_enc_control_id_AV1E_SET_MAX_REFERENCE_FRAMES: aome_enc_control_id = 124;
pub const aome_enc_control_id_AV1E_SET_MIN_GF_INTERVAL: aome_enc_control_id = 49;
pub const aome_enc_control_id_AV1E_SET_MODE_COST_UPD_FREQ: aome_enc_control_id = 127;
pub const aome_enc_control_id_AV1E_SET_MV_COST_UPD_FREQ: aome_enc_control_id = 128;
pub const aome_enc_control_id_AV1E_SET_NOISE_SENSITIVITY: aome_enc_control_id = 42;
pub const aome_enc_control_id_AV1E_SET_ROW_MT: aome_enc_control_id = 32;
pub const aome_enc_control_id_AV1E_SET_SUPERBLOCK_SIZE: aome_enc_control_id = 56;
pub const aome_enc_control_id_AV1E_SET_TILE_COLUMNS: aome_enc_control_id = 33;
pub const aome_enc_control_id_AV1E_SET_TILE_ROWS: aome_enc_control_id = 34;
pub const aome_enc_control_id_AV1E_SET_TRANSFER_CHARACTERISTICS: aome_enc_control_id = 46;
pub const aome_enc_control_id_AV1E_SET_TUNE_CONTENT: aome_enc_control_id = 43;

// -----------------------------------------------------------------------------
// extern 関数
// -----------------------------------------------------------------------------
unsafe extern "C" {
    pub fn aom_codec_av1_cx() -> *const aom_codec_iface;
    pub fn aom_codec_av1_dx() -> *const aom_codec_iface;
    pub fn aom_codec_control(
        ctx: *mut aom_codec_ctx_t,
        ctrl_id: ::std::os::raw::c_int,
        ...
    ) -> aom_codec_err_t;
    pub fn aom_codec_dec_init_ver(
        ctx: *mut aom_codec_ctx_t,
        iface: *const aom_codec_iface,
        cfg: *const aom_codec_dec_cfg_t,
        flags: aom_codec_flags_t,
        ver: ::std::os::raw::c_int,
    ) -> aom_codec_err_t;
    pub fn aom_codec_decode(
        ctx: *mut aom_codec_ctx_t,
        data: *const u8,
        data_sz: usize,
        user_priv: *mut ::std::os::raw::c_void,
    ) -> aom_codec_err_t;
    pub fn aom_codec_destroy(ctx: *mut aom_codec_ctx_t) -> aom_codec_err_t;
    pub fn aom_codec_enc_config_default(
        iface: *const aom_codec_iface,
        cfg: *mut aom_codec_enc_cfg_t,
        usage: ::std::os::raw::c_uint,
    ) -> aom_codec_err_t;
    pub fn aom_codec_enc_config_set(
        ctx: *mut aom_codec_ctx_t,
        cfg: *const aom_codec_enc_cfg_t,
    ) -> aom_codec_err_t;
    pub fn aom_codec_enc_init_ver(
        ctx: *mut aom_codec_ctx_t,
        iface: *const aom_codec_iface,
        cfg: *const aom_codec_enc_cfg_t,
        flags: aom_codec_flags_t,
        ver: ::std::os::raw::c_int,
    ) -> aom_codec_err_t;
    pub fn aom_codec_encode(
        ctx: *mut aom_codec_ctx_t,
        img: *const aom_image_t,
        pts: aom_codec_pts_t,
        duration: ::std::os::raw::c_ulong,
        flags: aom_enc_frame_flags_t,
    ) -> aom_codec_err_t;
    pub fn aom_codec_err_to_string(err: aom_codec_err_t) -> *const ::std::os::raw::c_char;
    pub fn aom_codec_error_detail(ctx: *const aom_codec_ctx_t) -> *const ::std::os::raw::c_char;
    pub fn aom_codec_get_cx_data(
        ctx: *mut aom_codec_ctx_t,
        iter: *mut aom_codec_iter_t,
    ) -> *const aom_codec_cx_pkt_t;
    pub fn aom_codec_get_frame(
        ctx: *mut aom_codec_ctx_t,
        iter: *mut aom_codec_iter_t,
    ) -> *mut aom_image_t;
    pub fn aom_img_alloc(
        img: *mut aom_image_t,
        fmt: aom_img_fmt_t,
        d_w: ::std::os::raw::c_uint,
        d_h: ::std::os::raw::c_uint,
        align: ::std::os::raw::c_uint,
    ) -> *mut aom_image_t;
    pub fn aom_img_free(img: *mut aom_image_t);
}
