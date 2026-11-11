//! セルの境界を格子上で追跡し、Douglas-Peucker で簡略化して三角形に分ける。

use std::collections::BTreeSet;

use charon_core::cell::{Cell, MaterialId};
use rapier2d::prelude::{Pose, SharedShape, Vector};

use crate::{BodyError, CELLS_PER_METER};

type Vertex = (i32, i32);
type Edge = (Vertex, Vertex);

fn occupied(cells: &[Cell], width: i32, height: i32, x: i32, y: i32) -> bool {
    x >= 0
        && y >= 0
        && x < width
        && y < height
        && cells[(y * width + x) as usize].material != MaterialId::EMPTY
}

/// 塗られたセルの隣が空なら、その 1 辺を時計回りの有向辺として取り出す。
fn edges(cells: &[Cell], width: i32, height: i32) -> BTreeSet<Edge> {
    let mut result = BTreeSet::new();
    for y in 0..height {
        for x in 0..width {
            if !occupied(cells, width, height, x, y) {
                continue;
            }
            if !occupied(cells, width, height, x, y - 1) {
                result.insert(((x, y), (x + 1, y)));
            }
            if !occupied(cells, width, height, x + 1, y) {
                result.insert(((x + 1, y), (x + 1, y + 1)));
            }
            if !occupied(cells, width, height, x, y + 1) {
                result.insert(((x + 1, y + 1), (x, y + 1)));
            }
            if !occupied(cells, width, height, x - 1, y) {
                result.insert(((x, y + 1), (x, y)));
            }
        }
    }
    result
}

fn direction(from: Vertex, to: Vertex) -> usize {
    match (to.0 - from.0, to.1 - from.1) {
        (1, 0) => 0,
        (0, 1) => 1,
        (-1, 0) => 2,
        (0, -1) => 3,
        _ => 0,
    }
}

pub(crate) fn trace(
    cells: &[Cell],
    width: i32,
    height: i32,
) -> Result<Vec<Vec<Vertex>>, BodyError> {
    let mut remaining = edges(cells, width, height);
    let mut contours = Vec::new();
    while let Some(&(start, end)) = remaining.iter().next() {
        remaining.remove(&(start, end));
        let mut contour = vec![start];
        let (mut previous, mut current) = (start, end);
        while current != start {
            contour.push(current);
            // 同じ頂点に 2 つの対角線状の輪が触れる場合は右回りを優先する。
            let incoming = direction(previous, current);
            let candidates = [(incoming + 1) % 4, incoming, (incoming + 3) % 4, (incoming + 2) % 4];
            let next = candidates.into_iter().find_map(|heading| {
                let point = match heading {
                    0 => (current.0 + 1, current.1),
                    1 => (current.0, current.1 + 1),
                    2 => (current.0 - 1, current.1),
                    _ => (current.0, current.1 - 1),
                };
                remaining.remove(&(current, point)).then_some(point)
            });
            let Some(next) = next else { return Err(BodyError::InvalidContour) };
            previous = current;
            current = next;
        }
        let simplified = simplify(&contour);
        // 許容誤差 1 セルでも細い壁が消える場合は元の格子輪郭を保つ。
        contours.push(if area(&simplified) == area(&contour) { simplified } else { contour });
    }
    Ok(contours)
}

fn line_distance_squared(point: Vertex, start: Vertex, end: Vertex) -> f64 {
    let (dx, dy) = ((end.0 - start.0) as f64, (end.1 - start.1) as f64);
    let (px, py) = ((point.0 - start.0) as f64, (point.1 - start.1) as f64);
    let length = dx * dx + dy * dy;
    if length == 0.0 {
        return px * px + py * py;
    }
    let t = ((px * dx + py * dy) / length).clamp(0.0, 1.0);
    (px - t * dx).powi(2) + (py - t * dy).powi(2)
}

fn simplify_open(points: &[Vertex], output: &mut Vec<Vertex>) {
    if points.len() < 3 {
        output.push(points[0]);
        return;
    }
    let (index, distance) = points[1..points.len() - 1]
        .iter()
        .enumerate()
        .map(|(i, &point)| {
            (i + 1, line_distance_squared(point, points[0], points[points.len() - 1]))
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap_or((0, 0.0));
    if distance > 1.0 {
        simplify_open(&points[..=index], output);
        simplify_open(&points[index..], output);
    } else {
        output.push(points[0]);
    }
}

fn simplify(points: &[Vertex]) -> Vec<Vertex> {
    if points.len() <= 3 {
        return points.to_vec();
    }
    let opposite = points
        .iter()
        .enumerate()
        .max_by_key(|(_, point)| (point.0 - points[0].0).pow(2) + (point.1 - points[0].1).pow(2))
        .map_or(points.len() / 2, |(i, _)| i);
    let mut result = Vec::new();
    simplify_open(&points[..=opposite], &mut result);
    let mut other = points[opposite..].to_vec();
    other.push(points[0]);
    simplify_open(&other, &mut result);
    result
}

fn area(points: &[Vertex]) -> i64 {
    points
        .iter()
        .zip(points.iter().cycle().skip(1))
        .take(points.len())
        .map(|(&(ax, ay), &(bx, by))| i64::from(ax) * i64::from(by) - i64::from(bx) * i64::from(ay))
        .sum()
}

fn contains(polygon: &[Vertex], point: Vertex) -> bool {
    let (x, y) = (point.0 as f64 + 0.1, point.1 as f64 + 0.1);
    polygon.iter().zip(polygon.iter().cycle().skip(1)).take(polygon.len()).fold(
        false,
        |inside, (&(ax, ay), &(bx, by))| {
            let (ay, by) = (ay as f64, by as f64);
            if (ay > y) != (by > y) && x < (bx - ax) as f64 * (y - ay) / (by - ay) + ax as f64 {
                !inside
            } else {
                inside
            }
        },
    )
}

/// 各外周に内周を割り当て、三角形の集合を rapier 用の compound にする。
pub(crate) fn triangulate(
    cells: &[Cell],
    width: u16,
    height: u16,
    origin: [f32; 2],
) -> Result<Vec<(Pose, SharedShape)>, BodyError> {
    let contours = trace(cells, i32::from(width), i32::from(height))?;
    let mut result = Vec::new();
    for (i, outer) in contours.iter().enumerate().filter(|(_, contour)| area(contour) > 0) {
        let mut vertices: Vec<Vertex> = outer.clone();
        let mut holes = Vec::new();
        for inner in contours.iter().filter(|contour| area(contour) < 0) {
            let owner = contours
                .iter()
                .enumerate()
                .filter(|(_, candidate)| area(candidate) > 0 && contains(candidate, inner[0]))
                .min_by_key(|(_, candidate)| area(candidate));
            if owner.is_some_and(|(index, _)| index == i) {
                holes.push(vertices.len());
                vertices.extend(inner.iter().copied());
            }
        }
        let mut coords = Vec::with_capacity(vertices.len() * 2);
        for &(x, y) in &vertices {
            coords.extend_from_slice(&[x as f64, y as f64]);
        }
        let indices = earcutr::earcut(&coords, &holes, 2).map_err(|_| BodyError::Triangulation)?;
        for triangle in indices.as_chunks::<3>().0 {
            let point = |index: usize| {
                let (x, y) = vertices[index];
                Vector::new(
                    (x as f32 - origin[0]) / CELLS_PER_METER,
                    (y as f32 - origin[1]) / CELLS_PER_METER,
                )
            };
            result.push((
                Pose::identity(),
                SharedShape::triangle(point(triangle[0]), point(triangle[1]), point(triangle[2])),
            ));
        }
    }
    if result.is_empty() { Err(BodyError::Triangulation) } else { Ok(result) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concave_and_hollow_contours() {
        let filled = Cell { material: MaterialId(2), ..Cell::default() };
        let mut l = vec![Cell::default(); 12];
        for i in [0, 1, 2, 3, 4, 8] {
            l[i] = filled;
        }
        let mut hollow = vec![filled; 25];
        for y in 1..4 {
            for x in 1..4 {
                hollow[y * 5 + x] = Cell::default();
            }
        }
        for (cells, width, height, expected) in [(&l, 4, 3, 6.0), (&hollow, 5, 5, 16.0)] {
            let triangles = triangulate(cells, width, height, [0.0, 0.0]).unwrap();
            let area: f32 =
                triangles.iter().map(|(_, shape)| shape.mass_properties(1.0).mass()).sum();
            assert!(
                (area * CELLS_PER_METER.powi(2) - expected).abs() < 0.001,
                "{width}x{height}: area = {} (expected {expected})",
                area * CELLS_PER_METER.powi(2)
            );
        }
    }
}
