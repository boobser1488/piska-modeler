//! Немного линейной алгебры без внешних крейтов: аффинное преобразование 3×3 + перенос.

pub type V3 = [f32; 3];

pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

pub fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn length(a: V3) -> f32 {
    dot(a, a).sqrt()
}

pub fn normalize(a: V3) -> Option<V3> {
    let l = length(a);
    (l > 1e-12 && l.is_finite()).then(|| [a[0] / l, a[1] / l, a[2] / l])
}

/// `p ↦ m·p + t` (матрица по строкам).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine {
    pub m: [[f32; 3]; 3],
    pub t: V3,
}

impl Affine {
    pub const IDENTITY: Affine = Affine { m: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], t: [0.0; 3] };

    pub fn translation(t: V3) -> Affine {
        Affine { t, ..Affine::IDENTITY }
    }

    pub fn scale(s: V3) -> Affine {
        Affine { m: [[s[0], 0.0, 0.0], [0.0, s[1], 0.0], [0.0, 0.0, s[2]]], t: [0.0; 3] }
    }

    /// Поворот углами Эйлера в градусах: сначала X, потом Y, потом Z — `Rz·Ry·Rx`
    /// (порядок `ZYX` three.js / Blockbench, `EulerRot::ZYX` glam). Правая тройка:
    /// положительный угол — против часовой стрелки, если смотреть с конца оси.
    pub fn euler_degrees(degrees: V3) -> Affine {
        let [x, y, z] = degrees.map(f32::to_radians);
        let (sx, cx) = x.sin_cos();
        let (sy, cy) = y.sin_cos();
        let (sz, cz) = z.sin_cos();
        let rx = [[1.0, 0.0, 0.0], [0.0, cx, -sx], [0.0, sx, cx]];
        let ry = [[cy, 0.0, sy], [0.0, 1.0, 0.0], [-sy, 0.0, cy]];
        let rz = [[cz, -sz, 0.0], [sz, cz, 0.0], [0.0, 0.0, 1.0]];
        Affine { m: mat_mul(rz, mat_mul(ry, rx)), t: [0.0; 3] }
    }

    /// Поворот `degrees` вокруг точки `pivot`.
    pub fn pivot(pivot: V3, degrees: V3) -> Affine {
        if degrees == [0.0; 3] {
            return Affine::IDENTITY;
        }
        Affine::translation(pivot).then_after(&Affine::euler_degrees(degrees)).then_after(&Affine::translation([-pivot[0], -pivot[1], -pivot[2]]))
    }

    /// `self ∘ inner`: сначала `inner`, потом `self`.
    pub fn then_after(&self, inner: &Affine) -> Affine {
        Affine { m: mat_mul(self.m, inner.m), t: add(self.apply_vector(inner.t), self.t) }
    }

    pub fn apply(&self, p: V3) -> V3 {
        add(self.apply_vector(p), self.t)
    }

    pub fn apply_vector(&self, v: V3) -> V3 {
        let m = &self.m;
        [
            m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
            m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
            m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
        ]
    }

    pub fn determinant(&self) -> f32 {
        let m = &self.m;
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    }
}

fn mat_mul(a: [[f32; 3]; 3], b: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let mut out = [[0.0; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            *cell = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: V3, b: V3) -> bool {
        length(sub(a, b)) < 1e-5
    }

    #[test]
    fn rotations_are_right_handed_and_x_first() {
        // +90° вокруг Y переводит +X в −Z, +90° вокруг X — +Y в +Z, вокруг Z — +X в +Y.
        assert!(close(Affine::euler_degrees([0.0, 90.0, 0.0]).apply([1.0, 0.0, 0.0]), [0.0, 0.0, -1.0]));
        assert!(close(Affine::euler_degrees([90.0, 0.0, 0.0]).apply([0.0, 1.0, 0.0]), [0.0, 0.0, 1.0]));
        assert!(close(Affine::euler_degrees([0.0, 0.0, 90.0]).apply([1.0, 0.0, 0.0]), [0.0, 1.0, 0.0]));
        // Порядок: X, потом Z. (0, 1, 0) –X90→ (0, 0, 1) –Z90→ (0, 0, 1);
        // в обратном порядке было бы (−1, 0, 0) –X90→ (−1, 0, 0).
        assert!(close(Affine::euler_degrees([90.0, 0.0, 90.0]).apply([0.0, 1.0, 0.0]), [0.0, 0.0, 1.0]));
        let p = Affine::pivot([1.0, 0.0, 0.0], [0.0, 0.0, 90.0]).apply([2.0, 0.0, 0.0]);
        assert!(close(p, [1.0, 1.0, 0.0]), "{p:?}");
    }
}
