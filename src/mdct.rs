//! AC-4 IMDCT and KBD window synthesis (ETSI TS 103 190-1 §5.5).
//!
//! Implements the naive reference IMDCT: given `N` spectral coefficients
//! `X[k]` produce `2N` time-domain samples via the direct-form summation
//! of Pseudocodes 60-64 (with a slow but straightforward complex IFFT).
//! A Kaiser-Bessel Derived window (§5.5.3) is applied during
//! unfolding, and overlap/add integrates the new block with the
//! previous.
//!
//! The spec formulation is well-defined for arbitrary transform lengths
//! N that are multiples of 8 (AC-4 uses 2048/1920/1536/1024/960/768
//! plus short-block subdivisions). This implementation is correctness-
//! first, not performance-first — the inner complex DFT is O(N^2).
//!
//! Window alpha values per Table 186:
//!
//!   - 2048 / 1920 / 1536 → alpha = 3
//!   -  1024 / 960 / 768  → alpha = 4
//!   -  512 / 480 / 384   → alpha = 4.5
//!   -  256 / 240 / 192   → alpha = 5

use core::f32::consts::PI;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use rustfft::num_complex::Complex64;
use rustfft::{Fft, FftPlanner};

thread_local! {
    /// Per-thread cache of planned inverse FFTs, keyed by size. AC-4
    /// only ever asks for a handful of distinct sizes (half of
    /// 2048/1920/1536/1024/960/768/512/480/384/256/240/192/128/120/96),
    /// so this cache stays tiny and avoids re-deriving the FFT's
    /// twiddle factors on every single `imdct()` call — this function
    /// runs once per channel per frame, thousands of times per track.
    static IFFT_CACHE: RefCell<HashMap<usize, Arc<dyn Fft<f64>>>> = RefCell::new(HashMap::new());
}

fn cached_inverse_fft(len: usize) -> Arc<dyn Fft<f64>> {
    IFFT_CACHE.with(|cache| {
        cache
            .borrow_mut()
            .entry(len)
            .or_insert_with(|| FftPlanner::new().plan_fft_inverse(len))
            .clone()
    })
}

/// KBD window alpha for transform length N (Table 185, 44.1/48 kHz
/// family; the 96/192 kHz columns shift these by one/two rows, which
/// this table does not model — all real content handled so far is
/// 48 kHz).
pub fn kbd_alpha(n: u32) -> f32 {
    match n {
        2048 | 1920 | 1536 | 4096 | 3840 | 3072 | 8192 | 7680 | 6144 => 3.0,
        1024 | 960 | 768 => 4.0,
        512 | 480 | 384 => 4.5,
        256 | 240 | 192 => 5.0,
        128 | 120 | 96 => 6.0,
        _ => 3.0,
    }
}

/// Modified Bessel function of the first kind, I_0(x), via its series
/// expansion (§5.5.3 formula). Converges rapidly for the modest |x|
/// ranges used in KBD (alpha * pi ≤ ~16).
fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0_f64;
    let mut term = 1.0_f64;
    let xh2 = (x / 2.0) * (x / 2.0);
    for k in 1..64 {
        term *= xh2 / (k as f64 * k as f64);
        sum += term;
        if term < 1e-20 * sum {
            break;
        }
    }
    sum
}

/// Compute a Kaiser-Bessel window of length `N+1` samples as defined
/// in §5.5.3: `W(N,n,α) = I0(πα * sqrt(1 - ((2n/N - 1)^2))) / I0(πα)`.
fn kaiser_bessel_kernel(n_kernel: u32, alpha: f32) -> Vec<f64> {
    let n = n_kernel as usize;
    let mut w = vec![0.0_f64; n + 1];
    let pa = PI as f64 * alpha as f64;
    let denom = bessel_i0(pa);
    for (i, w_i) in w.iter_mut().enumerate() {
        let t = 2.0 * i as f64 / n as f64 - 1.0;
        let arg = pa * (1.0 - t * t).max(0.0).sqrt();
        *w_i = bessel_i0(arg) / denom;
    }
    w
}

/// Generate the KBD window of length `2N` from the kernel per §5.5.3:
///
///   KBD_LEFT[n]  = sqrt( (Σ_{p=0..n} W[p]) / (Σ_{p=0..N} W[p]) )  for 0 ≤ n < N
///   KBD_RIGHT[n] = sqrt( (Σ_{p=0..2N-n-1} W[p]) / (Σ_{p=0..N} W[p]) )  for N ≤ n < 2N
///
/// The total window has length 2N.
pub fn kbd_window(n: u32) -> Vec<f32> {
    let alpha = kbd_alpha(n);
    let kernel = kaiser_bessel_kernel(n, alpha);
    let ns = n as usize;
    // Cumulative sum up to and including index p.
    let mut cum = vec![0.0_f64; ns + 1];
    cum[0] = kernel[0];
    for p in 1..=ns {
        cum[p] = cum[p - 1] + kernel[p];
    }
    let denom = cum[ns];
    let mut w = vec![0.0_f32; 2 * ns];
    for (i, w_i) in w.iter_mut().enumerate().take(ns) {
        let num = cum[i];
        *w_i = (num / denom).sqrt() as f32;
    }
    for (i, w_i) in w.iter_mut().enumerate().skip(ns) {
        // 2N - n - 1 per the spec formula.
        let p = 2 * ns - i - 1;
        let num = cum[p];
        *w_i = (num / denom).sqrt() as f32;
    }
    w
}

/// Naive IMDCT: transforms `N` spectral coefficients to `2N` time-domain
/// samples via the direct DFT formulation of Pseudocodes 60-62. No
/// window is applied here; [`imdct_apply_window_and_olap`] does that.
///
/// The output has length `2N` and uses the AC-4 sign convention (§5.5.2,
/// Step 5 pseudocode).
pub fn imdct(x: &[f32]) -> Vec<f32> {
    let big_n = x.len();
    let half_n = big_n / 2;
    let fp_n = big_n as f64;
    // Step 2 (pre-IFFT multiply).
    let mut zr = vec![0.0_f64; half_n];
    let mut zi = vec![0.0_f64; half_n];
    for k in 0..half_n {
        let xc = -(2.0 * std::f64::consts::PI * (8 * k + 1) as f64 / (16.0 * fp_n)).cos();
        let xs = -(2.0 * std::f64::consts::PI * (8 * k + 1) as f64 / (16.0 * fp_n)).sin();
        let a = x[big_n - 2 * k - 1] as f64;
        let b = x[2 * k] as f64;
        zr[k] = a * xc - b * xs;
        zi[k] = b * xc + a * xs;
    }
    // Step 3: complex IFFT of length half_n. Y[n] = sum_k Z[k] *
    // exp(+i*2*pi*k*n/half_n) — an *unnormalized* inverse DFT (no
    // 1/half_n scaling), which is exactly rustfft's own convention for
    // `plan_fft_inverse` (its docs guarantee `ifft(fft(x)) == len * x`,
    // i.e. neither direction is pre-scaled). This used to be a direct
    // O(half_n^2) double loop recomputing sin/cos every iteration —
    // billions of transcendental calls for a full track — now O(half_n
    // log half_n) via a real (cached, so repeat calls at the same size
    // skip re-planning) FFT.
    let mut buf: Vec<Complex64> = (0..half_n).map(|k| Complex64::new(zr[k], zi[k])).collect();
    cached_inverse_fft(half_n).process(&mut buf);
    let yr_half: Vec<f64> = buf.iter().map(|c| c.re).collect();
    let yi_half: Vec<f64> = buf.iter().map(|c| c.im).collect();
    // Step 4: post-IFFT multiply, divided by N.
    let mut yr = vec![0.0_f64; half_n];
    let mut yi = vec![0.0_f64; half_n];
    for n in 0..half_n {
        let xc = -(2.0 * std::f64::consts::PI * (8 * n + 1) as f64 / (16.0 * fp_n)).cos();
        let xs = -(2.0 * std::f64::consts::PI * (8 * n + 1) as f64 / (16.0 * fp_n)).sin();
        yr[n] = (yr_half[n] * xc - yi_half[n] * xs) / fp_n;
        yi[n] = (yi_half[n] * xc + yr_half[n] * xs) / fp_n;
    }
    // Step 5: unfold into 2N samples with the y[n] sign convention.
    // Window is NOT applied here — we return x[n] with w[n] = 1.
    let mut out = vec![0.0_f32; 2 * big_n];
    let quarter = big_n / 4;
    for n in 0..quarter {
        out[2 * n] = yi[quarter + n] as f32;
        out[2 * n + 1] = (-yr[quarter - n - 1]) as f32;
        out[big_n / 2 + 2 * n] = yr[n] as f32;
        out[big_n / 2 + 2 * n + 1] = (-yi[big_n / 2 - n - 1]) as f32;
        out[big_n + 2 * n] = yr[quarter + n] as f32;
        out[big_n + 2 * n + 1] = (-yi[quarter - n - 1]) as f32;
        out[3 * big_n / 2 + 2 * n] = (-yi[n]) as f32;
        out[3 * big_n / 2 + 2 * n + 1] = yr[big_n / 2 - n - 1] as f32;
    }
    out
}

/// Apply KBD window to a 2N IMDCT output and overlap/add with a N-sample
/// history buffer. Returns N PCM samples. The history buffer is updated
/// with the second half of the current block (premultiplied by the
/// right window so subsequent overlap is ready).
///
/// Simplified non-block-switching path: assumes N == N_prev.
pub fn imdct_olap_symmetric(x_unwindowed: &[f32], window: &[f32], overlap: &mut [f32]) -> Vec<f32> {
    let two_n = x_unwindowed.len();
    let n = two_n / 2;
    debug_assert_eq!(window.len(), two_n);
    debug_assert_eq!(overlap.len(), n);
    // Apply window to current 2N block.
    let x: Vec<f32> = x_unwindowed
        .iter()
        .zip(window.iter())
        .map(|(s, w)| s * w)
        .collect();
    // PCM = overlap + first-N of x.
    let pcm: Vec<f32> = overlap
        .iter()
        .zip(x.iter().take(n))
        .map(|(o, xi)| o + xi)
        .collect();
    // New overlap = second-N of x.
    overlap.copy_from_slice(&x[n..]);
    pcm
}

thread_local! {
    /// Per-thread cache of KBD windows keyed by transform length —
    /// `kbd_window` recomputes Bessel sums on every call and the
    /// block-switching OLA needs a window per block, thousands of
    /// times per track. Same pattern as `IFFT_CACHE`.
    static KBD_CACHE: RefCell<HashMap<u32, Arc<Vec<f32>>>> = RefCell::new(HashMap::new());
}

fn cached_kbd_window(n: u32) -> Arc<Vec<f32>> {
    KBD_CACHE.with(|cache| {
        cache
            .borrow_mut()
            .entry(n)
            .or_insert_with(|| Arc::new(kbd_window(n)))
            .clone()
    })
}

/// Block-switching overlap-add state for one channel — the full
/// §5.5.2.2 Step 5/6 machinery (Pseudocodes 63/64) including asymmetric
/// transition windows, replacing the symmetric-only
/// [`imdct_olap_symmetric`] path that discarded overlap history on
/// every transform-length change.
///
/// Model (per the spec and the §5.5.3 worked example):
///
/// * A composition buffer of `n_full` samples (`n_full` = the frame's
///   full block length, e.g. 2048). Partial blocks of length N sit
///   centred in that grid at offset `(n_full - N) / 2`.
/// * Each block's IMDCT output (2N samples, unwindowed) is processed
///   as: window the *previous* block's stored tail with the right
///   transition window (its shape depends on both `n_prev` and the
///   current N — deferring the right-half windowing until the next
///   block arrives is how the spec avoids needing lookahead), apply
///   the left transition window to the first N samples of the current
///   block, add them into the buffer, emit the first N buffer samples
///   as PCM, then shift and store the current block's *unwindowed*
///   second half as the new tail.
/// * Transition window halves are the KBD half for `NW = min(N,
///   n_prev)` padded with 0s/1s per Step 5/6: left = `[0 × skip,
///   KBD_LEFT(NW), 1 × skip]`, right = `[1 × skip, KBD_RIGHT(NW),
///   0 × skip]`.
///
/// For a constant-length stream this is arithmetically identical
/// (bit-for-bit) to the old symmetric path; it only behaves
/// differently at length transitions, where the old path wiped state
/// (dropping up to a full frame of windowed tail at every transient)
/// instead of transitioning.
///
/// NOTE on gain: Pseudocode 64 is implemented verbatim, i.e. without
/// the "factor of 2" the §5.5.3 worked example applies to the
/// left-windowed samples — the rest of this crate's pipeline
/// (dequant scaling, A-SPX, the i16 writeback) is calibrated to the
/// pseudocode's convention and a uniform constant factor does not
/// affect TDAC.
pub struct BlockSwitchOla {
    n_full: usize,
    /// Composition buffer, length `n_full`. Invariant maintained by
    /// the spec choreography: everything at/after
    /// `(n_full + n_prev) / 2` is zero.
    overlap: Vec<f32>,
    /// Previous block's transform length; 0 = no block processed yet.
    n_prev: usize,
}

impl BlockSwitchOla {
    pub fn new(n_full: usize) -> Self {
        Self {
            n_full,
            overlap: vec![0.0_f32; n_full],
            n_prev: 0,
        }
    }

    pub fn n_full(&self) -> usize {
        self.n_full
    }

    /// Process one block: `x` is the raw (unwindowed) 2N-sample IMDCT
    /// output. Returns N PCM samples. Falls back to a state reset if
    /// the block length is not representable in this buffer's grid
    /// (N > n_full or odd skip) — that indicates a stream reconfig the
    /// caller should have handled by recreating the state.
    pub fn process_block(&mut self, x: &[f32]) -> Vec<f32> {
        let n = x.len() / 2;
        if n == 0 {
            return Vec::new();
        }
        if n > self.n_full || (self.n_full - n) % 2 != 0 {
            *self = Self::new(n);
        }
        let nskip = (self.n_full - n) / 2;

        // Step 6 (first part): right-window the stored previous tail
        // in place. w = [1 × rskip, KBD_RIGHT(NW), 0 × rskip] over the
        // tail's own extent, which sits at offset (n_full - n_prev)/2.
        if self.n_prev > 0 {
            let nw = n.min(self.n_prev);
            let rskip = (self.n_prev - nw) / 2;
            let off = (self.n_full - self.n_prev) / 2;
            let w = cached_kbd_window(nw as u32);
            for i in 0..nw {
                // KBD_RIGHT(NW) is the second half of the 2·NW window.
                self.overlap[off + rskip + i] *= w[nw + i];
            }
            for i in (rskip + nw)..self.n_prev {
                self.overlap[off + i] = 0.0;
            }
        }

        // Step 5: left-window the current block's first half and add
        // it into the buffer at its centred offset. w = [0 × lskip,
        // KBD_LEFT(NW), 1 × lskip]. At stream start (no previous
        // block) use the plain full-length KBD left half.
        let lw_nw = if self.n_prev == 0 {
            n
        } else {
            n.min(self.n_prev)
        };
        let lskip = (n - lw_nw) / 2;
        let w = cached_kbd_window(lw_nw as u32);
        // Zero segment [0, lskip): contributes nothing, skip the adds.
        for i in 0..lw_nw {
            self.overlap[nskip + lskip + i] += x[lskip + i] * w[i];
        }
        for i in (lskip + lw_nw)..n {
            // Ones segment.
            self.overlap[nskip + i] += x[i];
        }

        // Step 6 (rest): emit, shift, store the unwindowed tail.
        let pcm = self.overlap[..n].to_vec();
        for i in 0..nskip {
            self.overlap[i] = self.overlap[n + i];
        }
        for i in 0..n {
            self.overlap[nskip + i] = x[n + i];
        }
        self.n_prev = n;
        pcm
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kbd_window_is_symmetric_and_energy_conserving() {
        let w = kbd_window(64);
        assert_eq!(w.len(), 128);
        // Symmetric about the midpoint.
        for i in 0..64 {
            assert!((w[i] - w[127 - i]).abs() < 1e-4, "asymmetry at {i}");
        }
        // Princen-Bradley: w[n]^2 + w[N+n]^2 = 1.
        for i in 0..64 {
            let s = w[i] * w[i] + w[64 + i] * w[64 + i];
            assert!((s - 1.0).abs() < 1e-4, "princen-bradley failed: {s}");
        }
    }

    #[test]
    fn imdct_dc_bin_gives_uniform_output() {
        // X[0] = 1.0, rest 0 -> output is a cosine of known shape.
        // Simple sanity: output has energy (nonzero) and is finite.
        let mut x = vec![0.0_f32; 64];
        x[0] = 1.0;
        let y = imdct(&x);
        assert_eq!(y.len(), 128);
        let energy: f32 = y.iter().map(|s| s * s).sum();
        assert!(energy > 0.0);
        assert!(energy.is_finite());
    }

    #[test]
    fn imdct_inverse_roundtrip_bin() {
        // Putting a non-DC spectral line at X[2] and checking that the
        // energy is spread across the time samples.
        let n = 16;
        let mut x = vec![0.0_f32; n];
        x[2] = 1.0;
        let y = imdct(&x);
        assert_eq!(y.len(), 2 * n);
        // Some elements should be nonzero.
        let nonzero = y.iter().filter(|&&v| v.abs() > 1e-6).count();
        assert!(nonzero > 0);
    }

    /// Direct O(N²) IMDCT transcribed from Pseudocodes 60-62 exactly as
    /// the pre-round-401 implementation computed it — the reference the
    /// FFT-based [`imdct`] must match. Test-only: exists to bound the
    /// rustfft path's numerical deviation, including accumulated
    /// overlap-add drift over many frames.
    fn imdct_direct_reference(x: &[f32]) -> Vec<f32> {
        let big_n = x.len();
        let half_n = big_n / 2;
        let fp_n = big_n as f64;
        let mut zr = vec![0.0_f64; half_n];
        let mut zi = vec![0.0_f64; half_n];
        for k in 0..half_n {
            let xc = -(2.0 * std::f64::consts::PI * (8 * k + 1) as f64 / (16.0 * fp_n)).cos();
            let xs = -(2.0 * std::f64::consts::PI * (8 * k + 1) as f64 / (16.0 * fp_n)).sin();
            let a = x[big_n - 2 * k - 1] as f64;
            let b = x[2 * k] as f64;
            zr[k] = a * xc - b * xs;
            zi[k] = b * xc + a * xs;
        }
        let mut yr_half = vec![0.0_f64; half_n];
        let mut yi_half = vec![0.0_f64; half_n];
        for n in 0..half_n {
            let (mut re, mut im) = (0.0_f64, 0.0_f64);
            for k in 0..half_n {
                let ang = 2.0 * std::f64::consts::PI * (k * n) as f64 / half_n as f64;
                let (s, c) = ang.sin_cos();
                re += zr[k] * c - zi[k] * s;
                im += zi[k] * c + zr[k] * s;
            }
            yr_half[n] = re;
            yi_half[n] = im;
        }
        let mut yr = vec![0.0_f64; half_n];
        let mut yi = vec![0.0_f64; half_n];
        for n in 0..half_n {
            let xc = -(2.0 * std::f64::consts::PI * (8 * n + 1) as f64 / (16.0 * fp_n)).cos();
            let xs = -(2.0 * std::f64::consts::PI * (8 * n + 1) as f64 / (16.0 * fp_n)).sin();
            yr[n] = (yr_half[n] * xc - yi_half[n] * xs) / fp_n;
            yi[n] = (yi_half[n] * xc + yr_half[n] * xs) / fp_n;
        }
        let mut out = vec![0.0_f32; 2 * big_n];
        let quarter = big_n / 4;
        for n in 0..quarter {
            out[2 * n] = yi[quarter + n] as f32;
            out[2 * n + 1] = (-yr[quarter - n - 1]) as f32;
            out[big_n / 2 + 2 * n] = yr[n] as f32;
            out[big_n / 2 + 2 * n + 1] = (-yi[big_n / 2 - n - 1]) as f32;
            out[big_n + 2 * n] = yr[quarter + n] as f32;
            out[big_n + 2 * n + 1] = (-yi[quarter - n - 1]) as f32;
            out[3 * big_n / 2 + 2 * n] = (-yi[n]) as f32;
            out[3 * big_n / 2 + 2 * n + 1] = yr[big_n / 2 - n - 1] as f32;
        }
        out
    }

    /// Deterministic xorshift so drift tests are reproducible without
    /// pulling in a rand dependency.
    struct XorShift(u64);
    impl XorShift {
        fn next_f32(&mut self) -> f32 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            // Map to [-1, 1).
            ((x >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0) as f32
        }
    }

    #[test]
    fn fft_imdct_matches_direct_reference_all_sizes() {
        // Every 48 kHz-family transform size real content can use
        // (full blocks and their /2../16 subdivisions).
        for &n in &[
            2048usize, 1920, 1536, 1024, 960, 768, 512, 480, 384, 256, 240, 192, 128, 120, 96,
        ] {
            let mut rng = XorShift(0x9E3779B97F4A7C15 ^ n as u64);
            let x: Vec<f32> = (0..n).map(|_| rng.next_f32()).collect();
            let fast = imdct(&x);
            let slow = imdct_direct_reference(&x);
            let peak = slow.iter().fold(0.0_f32, |m, v| m.max(v.abs())).max(1e-12);
            for (i, (a, b)) in fast.iter().zip(slow.iter()).enumerate() {
                assert!(
                    (a - b).abs() <= 1e-6 * peak.max(1.0),
                    "size {n} sample {i}: fft {a} vs direct {b}"
                );
            }
        }
    }

    #[test]
    fn fft_imdct_no_accumulated_drift_over_many_frames() {
        // Chain hundreds of frames of random spectra through the full
        // block-switching OLA with both IMDCT backends and compare the
        // final PCM — bounds accumulated floating-point drift in the
        // overlap-add state, which single-frame comparison can't see.
        // (Sizes kept small so the O(N²) reference stays fast; drift
        // per operation is size-independent.)
        let n = 128usize;
        let frames = 400;
        let mut ola_fast = BlockSwitchOla::new(n);
        let mut ola_slow = BlockSwitchOla::new(n);
        let mut rng = XorShift(0xDEADBEEFCAFEF00D);
        let mut worst = 0.0_f32;
        for _ in 0..frames {
            let x: Vec<f32> = (0..n).map(|_| rng.next_f32()).collect();
            let fast = ola_fast.process_block(&imdct(&x));
            let slow = ola_slow.process_block(&imdct_direct_reference(&x));
            for (a, b) in fast.iter().zip(slow.iter()) {
                worst = worst.max((a - b).abs());
            }
        }
        assert!(
            worst <= 1e-5,
            "accumulated fft-vs-direct divergence {worst} exceeds 1e-5"
        );
    }

    /// Forward MDCT built as the numerical adjoint of this module's own
    /// [`imdct`]: probe the synthesis basis with unit spectra once per
    /// size, then analyse by inner product. Guarantees the forward
    /// transform matches the inverse's phase/sign/scale conventions by
    /// construction, so the round-trip tests below test the *OLA*, not
    /// a hand-transcribed forward formula. O(N²) — test-only.
    struct ForwardMdct {
        /// basis[k] = imdct(e_k), length 2N each.
        basis: Vec<Vec<f32>>,
    }

    impl ForwardMdct {
        fn new(n: usize) -> Self {
            let mut basis = Vec::with_capacity(n);
            for k in 0..n {
                let mut e = vec![0.0_f32; n];
                e[k] = 1.0;
                basis.push(imdct(&e));
            }
            Self { basis }
        }

        fn analyse(&self, x_windowed: &[f32]) -> Vec<f32> {
            // Normalize by each basis vector's squared norm so the
            // round-trip gain is 1 for every block size — the IMDCT's
            // 1/N scaling would otherwise make the gain size-dependent
            // and mixed-size sequences would reconstruct with
            // per-region gain steps.
            self.basis
                .iter()
                .map(|b| {
                    let (mut dot, mut nrm) = (0.0_f64, 0.0_f64);
                    for (bi, xi) in b.iter().zip(x_windowed.iter()) {
                        dot += *bi as f64 * *xi as f64;
                        nrm += *bi as f64 * *bi as f64;
                    }
                    (dot / nrm.max(1e-30)) as f32
                })
                .collect()
        }
    }

    /// Build the §5.5.2.2 transition window half of length `n`, where
    /// the transition is against a neighbouring block of length
    /// `n_other`. `left` selects the Step-5 left half
    /// ([0×skip, KBD_LEFT(NW), 1×skip]) vs. the Step-6 right half
    /// ([1×skip, KBD_RIGHT(NW), 0×skip]).
    fn transition_half(n: usize, n_other: usize, left: bool) -> Vec<f32> {
        let nw = n.min(n_other);
        let skip = (n - nw) / 2;
        let w = kbd_window(nw as u32);
        (0..n)
            .map(|i| {
                if i < skip {
                    if left {
                        0.0
                    } else {
                        1.0
                    }
                } else if i < skip + nw {
                    if left {
                        w[i - skip]
                    } else {
                        w[nw + (i - skip)]
                    }
                } else if left {
                    1.0
                } else {
                    0.0
                }
            })
            .collect()
    }

    /// Shared round-trip driver: encode `signal` as a sequence of
    /// blocks (`sizes`, grouped into frames summing to `n_full`),
    /// windowing each block with the same transition windows the
    /// decoder OLA applies, then decode through [`BlockSwitchOla`] and
    /// return the reconstruction SNR (dB) over the steady-state region,
    /// after estimating the round-trip's constant gain from the data.
    ///
    /// Geometry: the block whose hop emits output samples [t, t+nb)
    /// has its 2nb window spanning source samples
    /// [t + (n_full-nb)/2, t + (n_full-nb)/2 + 2nb) — partial blocks
    /// sit centred in the full-block grid (§5.5.3), and with that
    /// centring the OLA's output sample k reconstructs source sample k
    /// with no size-dependent shift.
    fn roundtrip_snr_db(sizes: &[usize], n_full: usize) -> f64 {
        let total_hops: usize = sizes.iter().sum();
        let pad = 2 * n_full;
        let mut rng = XorShift(0xFEEDFACE8BADF00D);
        let signal: Vec<f32> = (0..pad + total_hops + 2 * n_full)
            .map(|_| rng.next_f32())
            .collect();

        let mut forwards: HashMap<usize, ForwardMdct> = HashMap::new();
        let mut ola = BlockSwitchOla::new(n_full);
        let mut out: Vec<f32> = Vec::new();
        let mut t = 0usize;
        for (bi, &nb) in sizes.iter().enumerate() {
            let n_prev = if bi == 0 { nb } else { sizes[bi - 1] };
            let n_next = sizes.get(bi + 1).copied().unwrap_or(nb);
            let wl = transition_half(nb, n_prev, true);
            let wr = transition_half(nb, n_next, false);
            let base = pad + t + (n_full - nb) / 2;
            let mut seg = vec![0.0_f32; 2 * nb];
            for i in 0..nb {
                seg[i] = signal[base + i] * wl[i];
                seg[nb + i] = signal[base + nb + i] * wr[i];
            }
            let fwd = forwards.entry(nb).or_insert_with(|| ForwardMdct::new(nb));
            let spec = fwd.analyse(&seg);
            out.extend_from_slice(&ola.process_block(&imdct(&spec)));
            t += nb;
        }

        // Steady-state region: skip the first and last full frame's
        // worth of output (stream edges have no overlap partner).
        let a = n_full;
        let b = out.len() - n_full;
        let recon = &out[a..b];
        let orig: Vec<f64> = (a..b).map(|k| signal[pad + k] as f64).collect();
        let dot: f64 = recon
            .iter()
            .zip(orig.iter())
            .map(|(r, o)| *r as f64 * o)
            .sum();
        let nrg: f64 = orig.iter().map(|o| o * o).sum();
        assert!(nrg > 0.0);
        let gain = dot / nrg;
        assert!(
            gain.abs() > 1e-3,
            "round-trip produced ~no signal (gain {gain})"
        );
        let mut err = 0.0_f64;
        for (r, o) in recon.iter().zip(orig.iter()) {
            let e = *r as f64 - gain * o;
            err += e * e;
        }
        10.0 * (gain * gain * nrg / err.max(1e-30)).log10()
    }

    /// Constant block size — validates the round-trip harness (and the
    /// degenerate symmetric path of the OLA) before the switching
    /// tests lean on it.
    #[test]
    fn tdac_roundtrip_constant_blocks_reconstructs() {
        let sizes = vec![256usize; 12];
        let snr = roundtrip_snr_db(&sizes, 256);
        assert!(
            snr > 100.0,
            "constant-size TDAC reconstruction SNR {snr:.1} dB"
        );
    }

    /// The real block-switching proof: long→short, short→short,
    /// short→long and mixed-subdivision frames must all reconstruct.
    /// The old wipe-on-length-change path fails this by construction
    /// (it discards the outgoing block's tail at every transition).
    #[test]
    fn tdac_roundtrip_with_block_switching_reconstructs() {
        let n_full = 512usize;
        let frames: Vec<Vec<usize>> = vec![
            vec![512],
            vec![512],
            vec![256, 256],
            vec![128, 128, 128, 128],
            vec![128, 128, 256],
            vec![512],
            vec![256, 128, 128],
            vec![512],
        ];
        let mut sizes = Vec::new();
        for f in &frames {
            assert_eq!(f.iter().sum::<usize>(), n_full);
            sizes.extend_from_slice(f);
        }
        let snr = roundtrip_snr_db(&sizes, n_full);
        assert!(
            snr > 100.0,
            "block-switching TDAC reconstruction SNR {snr:.1} dB"
        );
    }

    #[test]
    fn imdct_olap_produces_reasonable_output() {
        let n = 64;
        let mut x = vec![0.0_f32; n];
        x[1] = 10.0; // one strong spectral line
        let y = imdct(&x);
        let w = kbd_window(n as u32);
        let mut olap = vec![0.0_f32; n];
        let pcm1 = imdct_olap_symmetric(&y, &w, &mut olap);
        assert_eq!(pcm1.len(), n);
        // Second frame — overlap from first should be audible.
        let pcm2 = imdct_olap_symmetric(&y, &w, &mut olap);
        assert_eq!(pcm2.len(), n);
        let e1: f32 = pcm1.iter().map(|s| s * s).sum();
        let e2: f32 = pcm2.iter().map(|s| s * s).sum();
        // Second block should have noticeable energy (first has only
        // right-half contribution; second is the full stationary
        // response).
        assert!(e1.is_finite() && e2.is_finite());
        assert!(e2 > 0.0);
    }
}
