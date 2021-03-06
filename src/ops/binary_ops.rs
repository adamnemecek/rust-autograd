/// Implement +, -, *, / operators for Tensor
/// +=, -=, *=, /= are provided as methods of ops::inplace_*.
/// *=, /= don't propagate gradients.
use ndarray;
use ndarray_ext::NdArray;
use op;
use ops;
use std::mem;
use std::ops::{Add, Div, Mul, Sub};
use tensor::Tensor;

pub struct AddOp;
pub struct SubOp;
pub struct MulOp;
pub struct DivOp;
pub struct InplaceAddOp;
pub struct InplaceSubOp;
pub struct InplaceMulOp;
pub struct InplaceDivOp;
pub struct PreprocessBinOpGrad;
pub struct PreprocessBinOpGradGrad;

impl op::Op for PreprocessBinOpGrad {
    fn name(&self) -> &str {
        "PreprocessBinOpGrad"
    }

    // Computes x's gradient.
    // Involves reduction as necessary.
    // Inputs: [gy, target_shape]
    fn compute(&self, ctx: ::runtime::OpComputeContext) -> op::ComputeResult {
        let xs = ctx.grab_inputs();
        let gy = xs[0];
        let x_shape_ = ::ndarray_ext::vec_as_shape(xs[1]);
        let x_shape = x_shape_.as_slice();
        let gy_shape = gy.shape();

        let ret = if x_shape == gy_shape {
            // The case where forward path didn't cause broadcast.
            Err(::op::ComputeException::Delegate { to: 0 })
        } else {
            // Broadcast occurred. We need reduction of `gy`.
            // First, handle the case where x is scalar.
            let x_is_scalar = ::ndarray_ext::is_scalar_shape(x_shape);
            let x_shape = if x_is_scalar {
                vec![1; gy_shape.len()]
            } else {
                x_shape.to_vec()
            };
            // Reduce each dim as necessary
            let mut folded: Option<NdArray> = None;
            for (i, (x_axis, gy_axis)) in x_shape.iter().zip(gy_shape).enumerate() {
                if x_axis < gy_axis {
                    if *x_axis == 1 {
                        // `fold_axis` squashes the axis automatically.
                        let axis = ndarray::Axis(if x_is_scalar { 0 } else { i });
                        let ret = folded.as_ref().unwrap_or(gy).fold_axis(
                            axis.clone(),
                            0.,
                            |a, b| a.clone() + b.clone(),
                        );
                        if x_is_scalar {
                            mem::swap(&mut folded, &mut Some(ret));
                        } else {
                            // Expands squashed axis.
                            mem::swap(&mut folded, &mut Some(::ndarray_ext::expand_dims(ret, i)));
                        }
                    } else {
                        panic!("{}'s axis {} don't broadcast", ctx.grab_input_node(0), i);
                    }
                }
                // case of x_axis < gy_axis: unreachable
                // case of x_axis == gy_axis: nothing to do
            }
            // TODO
            Ok(folded.unwrap())
        };
        vec![ret]
    }

    // Do broadcast
    fn grad(&self, gy: &Tensor, inputs: &[&Tensor], _: &Tensor) -> Vec<Option<Tensor>> {
        let x_shape = inputs[1];
        let gx = Tensor::builder()
            .set_inputs(vec![gy, x_shape])
            .build(PreprocessBinOpGradGrad);
        vec![Some(gx), None]
    }
}

// Do broadcast if necessary.
// Inputs: [gy, target_shape]
impl op::Op for PreprocessBinOpGradGrad {
    fn name(&self) -> &str {
        "PreprocessBinOpGradGrad"
    }

    fn compute(&self, ctx: ::runtime::OpComputeContext) -> op::ComputeResult {
        let xs = ctx.grab_inputs();
        let gy = xs[0];
        let target_shape_ = xs[1];
        let target_shape_ = ::ndarray_ext::vec_as_shape(target_shape_);
        let target_shape = target_shape_.as_slice();

        if gy.shape() == target_shape {
            return vec![Err(::op::ComputeException::Delegate { to: 0 })];
        }

        let gy_is_scalar = ::ndarray_ext::is_scalar_shape(gy.shape());

        let ret = {
            let mut gy = gy.view();

            // make broadcast dims if needed
            if gy_is_scalar {
                for &axis in target_shape.iter() {
                    gy = ::ndarray_ext::expand_dims_view(gy, axis);
                }
            }

            // do broadcast
            if let Some(ret) = gy.broadcast(target_shape) {
                ret.to_owned()
            } else {
                panic!("Cant't broadcast.");
            }
        };

        vec![Ok(ret)]
    }

    fn grad(&self, gy: &Tensor, inputs: &[&Tensor], _: &Tensor) -> Vec<Option<Tensor>> {
        let gx = Tensor::builder()
            .set_inputs(vec![inputs[0], gy])
            .build(PreprocessBinOpGrad);
        vec![Some(gx), None]
    }
}

impl op::Op for AddOp {
    fn name(&self) -> &str {
        "Add"
    }

    fn compute(&self, ctx: ::runtime::OpComputeContext) -> op::ComputeResult {
        let xs = ctx.grab_inputs();
        add_forward(xs[0], xs[1])
    }

    fn grad(&self, gy: &Tensor, inputs: &[&Tensor], _: &Tensor) -> Vec<Option<Tensor>> {
        let (gy1, gy2) = preprocess_gy(inputs[0], inputs[1], gy);
        vec![Some(gy1), Some(gy2)]
    }
}

impl op::Op for SubOp {
    fn name(&self) -> &str {
        "Sub"
    }

    fn compute(&self, ctx: ::runtime::OpComputeContext) -> op::ComputeResult {
        let xs = ctx.grab_inputs();
        let x0 = xs[0];
        let x1 = xs[1];
        let shape0: &[usize] = x0.shape();
        let ret = if shape0 == &[] {
            // is scalar
            let x0_elem = x0[ndarray::IxDyn(&[])];
            Ok(x1.map(move |a| x0_elem - a))
        } else {
            Ok(x0 - x1)
        };
        vec![ret]
    }

    fn grad(&self, gy: &Tensor, inputs: &[&Tensor], _: &Tensor) -> Vec<Option<Tensor>> {
        let (gy1, gy2) = preprocess_gy(inputs[0], inputs[1], gy);
        vec![Some(gy1), Some(ops::neg(&gy2))]
    }
}

impl op::Op for MulOp {
    fn name(&self) -> &str {
        "Mul"
    }

    fn compute(&self, ctx: ::runtime::OpComputeContext) -> op::ComputeResult {
        let xs = ctx.grab_inputs();
        mul_forward(xs[0], xs[1])
    }

    fn grad(&self, gy: &Tensor, inputs: &[&Tensor], _: &Tensor) -> Vec<Option<Tensor>> {
        let x0 = inputs[0];
        let x1 = inputs[1];
        let (gy1, gy2) = preprocess_gy(x0, x1, gy);
        vec![Some(gy1 * x1), Some(gy2 * x0)]
    }
}

impl op::Op for DivOp {
    fn name(&self) -> &str {
        "Div"
    }

    fn compute(&self, ctx: ::runtime::OpComputeContext) -> op::ComputeResult {
        let xs = ctx.grab_inputs();
        let x0 = xs[0];
        let x1 = xs[1];
        let shape0: &[usize] = x0.shape();
        let shape1: &[usize] = x1.shape();
        let is_scalar0 = shape0 == &[] || shape0 == &[0];
        let is_scalar1 = shape1 == &[] || shape1 == &[1];
        let ret = if is_scalar0 {
            // a is a scalar
            let x0_elem = x0[ndarray::IxDyn(&[])];
            Ok(x1.map(move |a| x0_elem / a))
        } else if is_scalar1 {
            // b is a scalar
            let x1_elem = x1[ndarray::IxDyn(&[])];
            Ok(x0 * (1. / x1_elem))
        } else {
            Ok(x0 / x1)
        };
        vec![ret]
    }

    fn grad(&self, gy: &Tensor, inputs: &[&Tensor], _: &Tensor) -> Vec<Option<Tensor>> {
        let x0 = inputs[0];
        let x1 = inputs[1];
        let (gy1, gy2) = preprocess_gy(x0, x1, gy);
        vec![Some(gy1 / x1), Some(ops::neg(x0) * ops::pow(x1, -2.) * gy2)]
    }
}

impl op::Op for InplaceAddOp {
    fn name(&self) -> &str {
        "InplaceAdd"
    }

    fn compute(&self, mut ctx: ::runtime::OpComputeContext) -> op::ComputeResult {
        let xs = unsafe { ctx.grab_assignable_inputs() };
        // safe transmute probably
        let x1: &&NdArray = unsafe { mem::transmute(&mut xs[1]) };
        xs[0].zip_mut_with(x1, |a, &b| *a += b);
        vec![Err(::op::ComputeException::Delegate { to: 0 })]
    }

    fn grad(&self, gy: &Tensor, inputs: &[&Tensor], _: &Tensor) -> Vec<Option<Tensor>> {
        let (gy1, gy2) = preprocess_gy(inputs[0], inputs[1], gy);
        vec![Some(gy1), Some(gy2)]
    }
}

impl op::Op for InplaceSubOp {
    fn name(&self) -> &str {
        "InplaceSub"
    }

    fn compute(&self, mut ctx: ::runtime::OpComputeContext) -> op::ComputeResult {
        let xs = unsafe { ctx.grab_assignable_inputs() };
        // safe transmute probably
        let x1: &&NdArray = unsafe { mem::transmute(&mut xs[1]) };
        xs[0].zip_mut_with(x1, |a, &b| *a -= b);
        vec![Err(::op::ComputeException::Delegate { to: 0 })]
    }

    fn grad(&self, gy: &Tensor, inputs: &[&Tensor], _: &Tensor) -> Vec<Option<Tensor>> {
        let (gy1, gy2) = preprocess_gy(inputs[0], inputs[1], gy);
        vec![Some(gy1), Some(ops::neg(&gy2))]
    }
}

impl op::Op for InplaceMulOp {
    fn name(&self) -> &str {
        "InplaceMul"
    }

    fn compute(&self, mut ctx: ::runtime::OpComputeContext) -> op::ComputeResult {
        let xs = unsafe { ctx.grab_assignable_inputs() };
        // safe transmute probably
        let x1: &&NdArray = unsafe { mem::transmute(&mut xs[1]) };
        xs[0].zip_mut_with(x1, |a, &b| *a *= b);
        vec![Err(::op::ComputeException::Delegate { to: 0 })]
    }

    fn grad(&self, _: &Tensor, _: &[&Tensor], _: &Tensor) -> Vec<Option<Tensor>> {
        vec![None, None]
    }
}

impl op::Op for InplaceDivOp {
    fn name(&self) -> &str {
        "InplaceDiv"
    }

    fn compute(&self, mut ctx: ::runtime::OpComputeContext) -> op::ComputeResult {
        let xs = unsafe { ctx.grab_assignable_inputs() };
        // safe transmute probably
        let x1: &&NdArray = unsafe { mem::transmute(&mut xs[1]) };
        xs[0].zip_mut_with(x1, |a, &b| *a /= b);
        vec![Err(::op::ComputeException::Delegate { to: 0 })]
    }

    fn grad(&self, _: &Tensor, _: &[&Tensor], _: &Tensor) -> Vec<Option<Tensor>> {
        vec![None, None]
    }
}

// Reduce gy if broadcast occurred in the forward path.
fn preprocess_gy(x0: &Tensor, x1: &Tensor, gy: &Tensor) -> (Tensor, Tensor) {
    let shape0 = x0.shape();
    let shape1 = x1.shape();
    let gy0 = Tensor::builder()
        .set_inputs(vec![gy, &shape0])
        .set_shape(shape0)
        .build(PreprocessBinOpGrad);
    let gy1 = Tensor::builder()
        .set_inputs(vec![gy, &shape1])
        .set_shape(shape1)
        .build(PreprocessBinOpGrad);
    (gy0, gy1)
}

// -- std::ops::{Add, Sub, Mul, Div} implementations --

macro_rules! impl_bin_op_between_tensor_and_scalar {
    ($trt:ident, $func:ident, $op:ident, $scalar_type:ty) => {
        // scalar op Tensor
        impl $trt<Tensor> for $scalar_type {
            type Output = Tensor;
            fn $func(self, rhs: Tensor) -> Self::Output {
                Tensor::builder()
                    .set_inputs(vec![&ops::scalar(self as f32), &rhs])
                    .set_shape(rhs.shape())
                    .build($op)
            }
        }

        // scalar op &Tensor
        impl<'a> $trt<&'a Tensor> for $scalar_type {
            type Output = Tensor;
            fn $func(self, rhs: &'a Tensor) -> Self::Output {
                Tensor::builder()
                    .set_inputs(vec![&ops::scalar(self as f32), &rhs])
                    .set_shape(rhs.shape())
                    .build($op)
            }
        }

        // Tensor op scalar
        impl $trt<$scalar_type> for Tensor {
            type Output = Tensor;
            fn $func(self, rhs: $scalar_type) -> Self::Output {
                Tensor::builder()
                    .set_inputs(vec![&self, &ops::scalar(rhs as f32)])
                    .set_shape(self.shape())
                    .build($op)
            }
        }

        // &Tensor op scalar
        impl<'a> $trt<$scalar_type> for &'a Tensor {
            type Output = Tensor;
            fn $func(self, rhs: $scalar_type) -> Self::Output {
                Tensor::builder()
                    .set_inputs(vec![&self, &ops::scalar(rhs as f32)])
                    .set_shape(self.shape())
                    .build($op)
            }
        }
    };
}

macro_rules! impl_bin_op_between_tensors {
    ($trt:ident, $func:ident, $op:ident) => {
        // Tensor op Tensor
        impl $trt for Tensor {
            type Output = Tensor;
            fn $func(self, rhs: Tensor) -> Self::Output {
                ops::$func(&self, &rhs)
            }
        }

        // Tensor op &Tensor
        impl<'a> $trt<&'a Tensor> for Tensor {
            type Output = Tensor;
            fn $func(self, rhs: &Tensor) -> Self::Output {
                ops::$func(&self, rhs)
            }
        }

        // &Tensor op Tensor
        impl<'a> $trt<Tensor> for &'a Tensor {
            type Output = Tensor;
            fn $func(self, rhs: Tensor) -> Self::Output {
                ops::$func(&self, &rhs)
            }
        }

        // &Tensor op &Tensor
        // lifetime of the two tensors are unrelated
        impl<'a, 'b> $trt<&'a Tensor> for &'b Tensor {
            type Output = Tensor;
            fn $func(self, rhs: &Tensor) -> Self::Output {
                ops::$func(self, rhs)
            }
        }
    };
}

macro_rules! impl_bin_op_forward {
    ($forward_name:ident, $bin_op:tt) => {
        fn $forward_name(x0: &NdArray, x1: &NdArray) -> op::ComputeResult
        {
            let shape0: &[usize]  = x0.shape();
            let shape1: &[usize]  = x1.shape();
            let scalar_shape = &[];
            let scalar_shape1 = &[0];

            let x0_is_scalar = shape0 == scalar_shape || shape0 == scalar_shape1;
            let x1_is_scalar = shape1 == scalar_shape || shape1 == scalar_shape1;

            let ret = if x0_is_scalar && !x1_is_scalar {
                let elem = x0[ndarray::IxDyn(&[])];
                Ok(x1.map(move |a| a $bin_op elem ))
            } else if x1_is_scalar && !x0_is_scalar {
                let elem = x1[ndarray::IxDyn(&[])];
                Ok(x0.map(move |a| a $bin_op elem ))
            } else if !x0_is_scalar && !x1_is_scalar {
                let len0: usize = shape0.iter().product();
                let len1: usize = shape1.iter().product();
                if len0 > len1 {
                    Ok(x0 $bin_op x1)
                } else {
                    Ok(x1 $bin_op x0)
                }
            } else {
                Ok(x0 $bin_op x1)
            };
            vec![ret]
        }
    };
}

impl_bin_op_forward!(add_forward, +);
impl_bin_op_forward!(mul_forward, *);

impl_bin_op_between_tensors!(Add, add, AddOp);
impl_bin_op_between_tensors!(Sub, sub, SubOp);
impl_bin_op_between_tensors!(Mul, mul, MulOp);
impl_bin_op_between_tensors!(Div, div, DivOp);

impl_bin_op_between_tensor_and_scalar!(Add, add, AddOp, i32);
impl_bin_op_between_tensor_and_scalar!(Sub, sub, SubOp, i32);
impl_bin_op_between_tensor_and_scalar!(Mul, mul, MulOp, i32);
impl_bin_op_between_tensor_and_scalar!(Div, div, DivOp, i32);

impl_bin_op_between_tensor_and_scalar!(Add, add, AddOp, i64);
impl_bin_op_between_tensor_and_scalar!(Sub, sub, SubOp, i64);
impl_bin_op_between_tensor_and_scalar!(Mul, mul, MulOp, i64);
impl_bin_op_between_tensor_and_scalar!(Div, div, DivOp, i64);

impl_bin_op_between_tensor_and_scalar!(Add, add, AddOp, f32);
impl_bin_op_between_tensor_and_scalar!(Sub, sub, SubOp, f32);
impl_bin_op_between_tensor_and_scalar!(Mul, mul, MulOp, f32);
impl_bin_op_between_tensor_and_scalar!(Div, div, DivOp, f32);

impl_bin_op_between_tensor_and_scalar!(Add, add, AddOp, f64);
impl_bin_op_between_tensor_and_scalar!(Sub, sub, SubOp, f64);
impl_bin_op_between_tensor_and_scalar!(Mul, mul, MulOp, f64);
impl_bin_op_between_tensor_and_scalar!(Div, div, DivOp, f64);

impl_bin_op_between_tensor_and_scalar!(Add, add, AddOp, u32);
impl_bin_op_between_tensor_and_scalar!(Sub, sub, SubOp, u32);
impl_bin_op_between_tensor_and_scalar!(Mul, mul, MulOp, u32);
impl_bin_op_between_tensor_and_scalar!(Div, div, DivOp, u32);

impl_bin_op_between_tensor_and_scalar!(Add, add, AddOp, u64);
impl_bin_op_between_tensor_and_scalar!(Sub, sub, SubOp, u64);
impl_bin_op_between_tensor_and_scalar!(Mul, mul, MulOp, u64);
impl_bin_op_between_tensor_and_scalar!(Div, div, DivOp, u64);

impl_bin_op_between_tensor_and_scalar!(Add, add, AddOp, usize);
impl_bin_op_between_tensor_and_scalar!(Sub, sub, SubOp, usize);
impl_bin_op_between_tensor_and_scalar!(Mul, mul, MulOp, usize);
impl_bin_op_between_tensor_and_scalar!(Div, div, DivOp, usize);

impl_bin_op_between_tensor_and_scalar!(Add, add, AddOp, isize);
impl_bin_op_between_tensor_and_scalar!(Sub, sub, SubOp, isize);
impl_bin_op_between_tensor_and_scalar!(Mul, mul, MulOp, isize);
impl_bin_op_between_tensor_and_scalar!(Div, div, DivOp, isize);
