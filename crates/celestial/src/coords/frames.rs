pub trait Frame {}

pub struct ICRS;
pub struct J2000;
pub struct Topocentric;

impl Frame for ICRS {}
impl Frame for J2000 {}
impl Frame for Topocentric {}
