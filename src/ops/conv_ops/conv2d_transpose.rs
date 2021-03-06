use super::*;

pub struct Conv2DTranspose {
    pub pad: usize,
    pub stride: usize,
    pub dilation: usize,
}

pub struct Conv2DTransposeFilterGrad {
    pub pad: usize,
    pub stride: usize,
    pub dilation: usize,
}

impl ::op::Op for Conv2DTranspose {
    fn name(&self) -> &str {
        "Conv2DTranspose"
    }

    fn compute(&self, ctx: ::runtime::OpComputeContext) -> ::op::ComputeResult {
        let xs = ctx.grab_inputs();

        let gy: &NdArray = xs[0]; // (batch, ych, yh, yw)
        let w: &NdArray = xs[1]; // (ych, xch, kh, kw)
        let gy_shape = gy.shape();
        let f_shape = w.shape();

        let batch_size = gy_shape[0];
        let ych = gy_shape[1];
        let yh = gy_shape[2];
        let yw = gy_shape[3];

        let xch = f_shape[1];
        let kh = f_shape[2];
        let kw = f_shape[3];
        let xh = get_xh!(self, yh, kh);
        let xw = get_xw!(self, yw, kw);

        assert_eq!(
            gy_shape.len(),
            4,
            "ag::conv2d: Input must be 4D (got {:?})",
            gy_shape
        );
        assert_eq!(
            f_shape.len(),
            4,
            "ag::conv2d: Filter must be 4D (got {:?})",
            f_shape
        );
        assert_eq!(
            ych, f_shape[0],
            "ag::conv2d: Number of input channels ({:?}) must match second filter dim ({:?})",
            ych, f_shape[0]
        );

        // sgemm params
        let k = ych;
        let n = yh * yw;
        let m = kh * kw * xch;

        let num_elements_in_batch_gx = xch * xh * xw;
        let num_elements_in_batch_col = xch * kh * kw * yh * yw;

        let gy = unsafe { slice::from_raw_parts(gy.as_ptr(), gy.len()) };
        let w: &f32 = unsafe { &*w.as_ptr() };
        let col = alloc_uninitialized_buf(batch_size * num_elements_in_batch_col);
        // Col2im buffer must be initialized with zeros
        let gx = vec![0.; batch_size * num_elements_in_batch_gx];

        #[cfg(feature = "mkl")]
        {
            cblas_sgemm_batch_wrapper(
                true,
                false,
                m,
                n,
                k,
                &[1.],
                vec![w; batch_size],
                get_region_heads(batch_size, gy),
                &[0.],
                get_region_heads(batch_size, col.as_slice()),
                1,
                batch_size,
            );

            (0..batch_size).into_par_iter().for_each(|i| {
                // for each mini-batch
                let col_region_head = &col[i * num_elements_in_batch_col];
                let gx_region_head = &gx[i * num_elements_in_batch_gx];
                col2im(
                    col_region_head,
                    xch,
                    xh,
                    xw,
                    kh,
                    kw,
                    self.pad,
                    self.pad,
                    self.stride,
                    self.stride,
                    self.dilation,
                    self.dilation,
                    gx_region_head,
                );
            });
        }
        #[cfg(not(feature = "mkl"))]
        {
            let num_elements_in_batch_gy = ych * yh * yw;
            // fallback: parallel sgemm + col2im using rayon
            (0..batch_size).into_par_iter().for_each(|i| {
                // for each mini-batch
                let gy_region_head = &gy[i * num_elements_in_batch_gy];
                let col_region_head = &col[i * num_elements_in_batch_col];
                let gx_region_head = &gx[i * num_elements_in_batch_gx];
                sgemm(
                    true,
                    false,
                    w,
                    gy_region_head,
                    col_region_head,
                    m,
                    n,
                    k,
                    1.,
                    0.,
                );
                col2im(
                    col_region_head,
                    xch,
                    xh,
                    xw,
                    kh,
                    kw,
                    self.pad,
                    self.pad,
                    self.stride,
                    self.stride,
                    self.dilation,
                    self.dilation,
                    gx_region_head,
                );
            });
        }

        let gx = NdArray::from_shape_vec(ndarray::IxDyn(&[batch_size, xch, xh, xw]), gx);
        vec![Ok(gx.unwrap())]
    }

    fn grad(&self, gy: &Tensor, xs: &[&Tensor], _: &Tensor) -> Vec<Option<Tensor>> {
        let x = xs[0];
        let w = xs[1];

        let gx = Tensor::builder()
            .set_inputs(vec![gy, w])
            .build(super::conv2d::Conv2D {
                pad: self.pad,
                stride: self.stride,
                dilation: self.dilation,
            });

        let gw = Tensor::builder()
            .set_inputs(vec![gy, x, &::ops::stop_gradient(w)])
            .build(Conv2DTransposeFilterGrad {
                pad: self.pad,
                stride: self.stride,
                dilation: self.dilation,
            });

        vec![Some(gx), Some(gw)]
    }
}

impl ::op::Op for Conv2DTransposeFilterGrad {
    fn name(&self) -> &str {
        "Conv2DTransposeFilterGrad"
    }

    fn compute(&self, ctx: ::runtime::OpComputeContext) -> ::op::ComputeResult {
        let xs = ctx.grab_inputs();
        let gy = xs[0];
        let x = xs[1];
        let k_shape = xs[2].shape();

        let x_shape = x.shape();
        let gy_shape = gy.shape();

        let batch_size = x_shape[0];
        let (kh, kw) = (k_shape[2], k_shape[3]);

        let num_elements_in_batch_g = { gy_shape[1] * gy_shape[2] * gy_shape[3] };
        let num_elements_in_batch_c = {
            get_yh!(self, gy_shape[2], kh) * get_yw!(self, gy_shape[3], kw) * kh * kw * gy_shape[1]
        };
        let num_elements_in_batch_x = x_shape[1] * x_shape[2] * x_shape[3];

        // sgemm params
        let m = x_shape[1];
        let n = kh * kw * gy_shape[1];
        let k = get_yh!(self, gy_shape[2], kh) * get_yw!(self, gy_shape[3], kw);

        let x = unsafe { slice::from_raw_parts(x.as_ptr(), x.len()) };
        let gy = unsafe { slice::from_raw_parts(gy.as_ptr(), gy.len()) };
        let cols = alloc_uninitialized_buf(batch_size * num_elements_in_batch_c);
        let gw = alloc_uninitialized_buf(k_shape[0] * k_shape[1] * k_shape[2] * k_shape[3]);
        let gw_head = unsafe { &*gw.as_ptr() };

        (0..batch_size).into_par_iter().for_each(|i| {
            let c_region_head = &cols[i * num_elements_in_batch_c];
            let g_region_head = &gy[i * num_elements_in_batch_g];
            im2col(
                g_region_head,
                gy_shape[1],
                gy_shape[2],
                gy_shape[3],
                kh,
                kw,
                self.pad,
                self.pad,
                self.stride,
                self.stride,
                self.dilation,
                self.dilation,
                c_region_head,
            );
        });

        for i in 0..batch_size {
            let x_region_head = &x[i * num_elements_in_batch_x];
            let c_region_head = &cols[i * num_elements_in_batch_c];
            sgemm(
                false,
                true,
                x_region_head,
                c_region_head,
                gw_head,
                m,
                n,
                k,
                1.,
                (i != 0) as i32 as f32,
            );
        }

        vec![Ok(NdArray::from_shape_vec(k_shape, gw).unwrap())]
    }

    fn grad(&self, gw: &Tensor, xs: &[&Tensor], _: &Tensor) -> Vec<Option<Tensor>> {
        let gy = xs[0];
        let x = xs[1];

        let ggy = Tensor::builder()
            .set_inputs(vec![x, gw])
            .build(Conv2DTranspose {
                pad: self.pad,
                stride: self.stride,
                dilation: self.dilation,
            });

        let ggx = Tensor::builder()
            .set_inputs(vec![gy, gw])
            .build(super::conv2d::Conv2D {
                pad: self.pad,
                stride: self.stride,
                dilation: self.dilation,
            });

        vec![Some(ggy), Some(ggx), None]
    }
}

#[test]
fn test_tensor_size_after_convolution_t() {
    let op = Conv2DTranspose {
        pad: 0,
        stride: 1,
        dilation: 1,
    };
    let (yh, yw) = (2, 2);
    let (kh, kw) = (2, 2);
    let xh = get_xh!(&op, yh, kh);
    let xw = get_xw!(&op, yw, kw);
    assert_eq!(xh, 3);
    assert_eq!(xw, 3);
}

#[test]
fn test_parallel_col2im() {
    let batch_size = 2;
    let op = Conv2DTranspose {
        pad: 0,
        stride: 1,
        dilation: 1,
    };
    let xch = 3;
    let (yh, yw) = (2, 2);
    let (kh, kw) = (2, 2);
    let xh = get_xh!(&op, yh, kh);
    let xw = get_xw!(&op, yw, kw);

    let num_elements_in_batch_col = xch * kh * kw * yh * yw;
    let num_elements_in_batch_im = xch * xh * xw;
    let cols = vec![2f32; 108 * batch_size];
    let im = vec![0f32; batch_size * xch * xh * xw];

    (0..batch_size).into_par_iter().for_each(|i| unsafe {
        let cols_head = (&cols[i * num_elements_in_batch_col]) as *const f32;
        let im_head = (&im[i * num_elements_in_batch_im]) as *const f32;
        col2im_cpu(
            cols_head,
            xch as i32,
            xh as i32,
            xw as i32,
            kh as i32,
            kw as i32,
            op.pad as i32,
            op.pad as i32,
            op.stride as i32,
            op.stride as i32,
            op.dilation as i32,
            op.dilation as i32,
            im_head,
        );
    });

    assert_eq!(
        im,
        vec![
            2.0, 4.0, 2.0, 4.0, 8.0, 4.0, 2.0, 4.0, 2.0, 2.0, 4.0, 2.0, 4.0, 8.0, 4.0, 2.0, 4.0,
            2.0, 2.0, 4.0, 2.0, 4.0, 8.0, 4.0, 2.0, 4.0, 2.0, 2.0, 4.0, 2.0, 4.0, 8.0, 4.0, 2.0,
            4.0, 2.0, 2.0, 4.0, 2.0, 4.0, 8.0, 4.0, 2.0, 4.0, 2.0, 2.0, 4.0, 2.0, 4.0, 8.0, 4.0,
            2.0, 4.0, 2.0,
        ]
    );
}

#[test]
fn test_deconv() {
    use op::Op;
    let op = Conv2DTranspose {
        pad: 0,
        stride: 1,
        dilation: 1,
    };
    let (kh, kw) = (2, 2);
    let (xch, ych) = (3, 2);
    let (yh, yw) = (2, 2);
    let (xh, xw) = (3, 3);
    let batch_size = 2;

    let w = ::ndarray_ext::ones(&[ych, xch, kh, kw]);
    let g = ::ndarray_ext::ones(&[batch_size, ych, yh, yw]);

    let ret = op.compute(::runtime::OpComputeContext::new(
        &::ops::zeros(&[0]), // dummy (not used)
        vec![&g, &w],
    ));

    let x = ::ndarray_ext::ones(&[batch_size, xch, xh, xw]);
    assert_eq!(x.shape(), ret[0].as_ref().unwrap().shape());

    assert_eq!(
        ret[0].clone().unwrap().into_raw_vec(),
        vec![
            2.0, 4.0, 2.0, 4.0, 8.0, 4.0, 2.0, 4.0, 2.0, 2.0, 4.0, 2.0, 4.0, 8.0, 4.0, 2.0, 4.0,
            2.0, 2.0, 4.0, 2.0, 4.0, 8.0, 4.0, 2.0, 4.0, 2.0, 2.0, 4.0, 2.0, 4.0, 8.0, 4.0, 2.0,
            4.0, 2.0, 2.0, 4.0, 2.0, 4.0, 8.0, 4.0, 2.0, 4.0, 2.0, 2.0, 4.0, 2.0, 4.0, 8.0, 4.0,
            2.0, 4.0, 2.0,
        ]
    )
}
