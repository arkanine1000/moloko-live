//! Index -> BGRX conversion of the changed canvas pixels, at native size. Scaling is left to the X server.

use crate::geometry::Rect;

/// Canvas rectangle `r` of the composited `frame` -> BGRX rows at the start of `buf`.
pub fn convert(frame: &[u8], canvas_width: usize, r: Rect, lut: &[[u8; 4]; 256], buf: &mut [u8]) {
    for (row, dst) in buf.chunks_exact_mut(r.width() * 4).take(r.height()).enumerate() {
        let src = &frame[(r.y0 + row) * canvas_width + r.x0..][..r.width()];
        for (px, &index) in dst.chunks_exact_mut(4).zip(src) {
            px.copy_from_slice(&lut[index as usize]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_a_rectangle_through_the_lut() {
        let mut lut = [[0u8; 4]; 256];
        lut[1] = [1, 2, 3, 0];
        lut[2] = [4, 5, 6, 0];
        let frame = [0, 1, 2, 0, 2, 1, 0, 0, 0];
        let mut buf = vec![9u8; 16];
        convert(
            &frame,
            3,
            Rect {
                x0: 1,
                y0: 0,
                x1: 3,
                y1: 2,
            },
            &lut,
            &mut buf,
        );
        assert_eq!(buf, [1, 2, 3, 0, 4, 5, 6, 0, 4, 5, 6, 0, 1, 2, 3, 0]);
    }
}
