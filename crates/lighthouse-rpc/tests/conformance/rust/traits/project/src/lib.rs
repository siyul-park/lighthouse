/// A shape.
pub trait Shape {
    /// Surface of the shape.
    fn area(&self) -> f64;

    fn describe(&self) -> String {
        format!("{}", self.area())
    }
}

pub trait Solid: Shape {
    fn volume(&self) -> f64;
}

pub struct Circle {
    pub r: f64,
}

impl Shape for Circle {
    fn area(&self) -> f64 {
        3.0 * self.r * self.r
    }
}

pub fn show(shape: &dyn Shape) -> String {
    shape.describe()
}

pub fn show_impl(shape: impl Shape) -> f64 {
    shape.area()
}

pub fn show_generic<S: Shape>(shape: S) -> f64 {
    shape.area()
}

pub fn show_where<S>(shape: S) -> f64
where
    S: Shape,
{
    shape.area()
}

pub fn show_path(circle: &Circle) -> f64 {
    Shape::area(circle)
}

pub struct Holder {
    inner: Box<dyn Shape>,
}

impl Holder {
    pub fn area(&self) -> f64 {
        self.inner.area()
    }
}
