extern crate ndarray;

use ndarray_ext::NdArray;
use std::collections::hash_map::HashMap;
use tensor::Tensor;


#[derive(Clone)]
/// What is necessary to run computation graphs.
///
/// `Context` object is used:
/// * to create shared variable tensors
/// * to create constant tensors
/// * to run computation graphs actually
///
/// When a computation graph is evaluated, all the variables/constants in the graph
/// must be generated in the same context; otherwise will panic.
///
/// ```
/// extern crate ndarray;
/// extern crate autograd as ag;
///
/// // new Context
/// let mut ctx = ag::Context::new();
///
/// let ref x = ag::placeholder(&[2]);
/// // make shared variable in the context
/// let ref v = ag::variable(ndarray::arr1(&[2., 2.]), &mut ctx);
/// let ref b = ag::ones(&[2]);
/// let ref z = x + v + b;
///
/// // fills placeholder
/// ctx.feed_input(x, ndarray::arr1(&[1., 1.]));
///
/// // eval
/// assert_eq!(z.eval(&mut ctx).as_slice().unwrap(), &[4., 4.]);
/// assert!(ctx.variables.contains_key(v));
/// assert_eq!(ctx.variables.get(v).unwrap(), &ndarray::arr1(&[2., 2.]).into_dyn());
/// ```
pub struct Context {
    /// Variables generated in this context.
    /// Each array can be obtained by using corresponding `Tensor` object.
    pub variables: HashMap<Tensor, NdArray>,

    #[doc(hidden)]
    // Evaluation results of tensors in this context are stored in this map.
    // Each output are cleared after evaluation.
    pub outputs: HashMap<Tensor, Result<NdArray, ::OpComputeErrorStatus>>,
}

impl Context {
    /// Creates new context object.
    pub fn new() -> Context
    {
        Context { variables: HashMap::new(), outputs: HashMap::new() }
    }

    /// Returns all variables in this context.
    pub fn list_vars(&self) -> Vec<&Tensor>
    {
        self.variables.keys().collect::<Vec<_>>()
    }

    /// Skips `arr`'s shape checking.
    pub fn feed_input_unchecked<T>(&mut self, placeholder: &Tensor, arr: ndarray::Array<f32, T>)
    where
        T: ndarray::Dimension,
    {
        if "PH" != placeholder.op.name() {
            panic!(
                "Don't call `feed_input_unchecked` with non placeholder, got: {}",
                placeholder.op.name()
            )
        }
        self.outputs.insert(placeholder.clone(), Ok(arr.into_dyn()));
    }

    // TODO: Input's shape checking for dynamic placeholder.
    pub fn feed_input<T>(&mut self, placeholder: &Tensor, arr: ndarray::Array<f32, T>)
    where
        T: ndarray::Dimension,
    {
        if "PH" != placeholder.op.name() {
            panic!(
                "Don't call `feed_input` with non placeholder, got: {}",
                placeholder.op.name()
            )
        }
        // check arr's shape
        if let Some(ref inner) = placeholder.shape {
            // unwrap is safe (guaranteed by ops::placeholder's implementation)
            assert_eq!(
                inner.eval(self).as_slice().unwrap(),
                arr.shape()
                    .iter()
                    .map(|&a| a as f32)
                    .collect::<Vec<_>>()
                    .as_slice()
            )
        }
        self.outputs.insert(placeholder.clone(), Ok(arr.into_dyn()));
    }

    /// Same as [autograd::variable](../ops/fn.variable.html).
    pub fn variable<T>(&mut self, arr: ndarray::Array<f32, T>) -> Tensor
        where
            T: ndarray::Dimension,
    {
        ::ops::variable(arr, self)
    }

    /// Same as [autograd::constant](../ops/fn.constant.html).
    pub fn constant<T>(&mut self, arr: ndarray::Array<f32, T>) -> Tensor
        where
            T: ndarray::Dimension,
    {
        ::ops::constant(arr, self)
    }
}
