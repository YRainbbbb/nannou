use crate::wgpu;

/// Reshapes a texture from its original size, sample_count and format to the destination size,
/// sample_count and format.
///
/// The `src_texture` must have the `TextureUsage::SAMPLED` enabled.
///
/// The `dst_texture` must have the `TextureUsage::OUTPUT_ATTACHMENT` enabled.
#[derive(Debug)]
pub struct Reshaper {
    _vs_mod: wgpu::ShaderModule,
    _fs_mod: wgpu::ShaderModule,
    bind_group_layout: wgpu::BindGroupLayout,
    bind_group: wgpu::BindGroup,
    render_pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    uniform_buffer: Option<wgpu::Buffer>,
    vertex_buffer: wgpu::Buffer,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd)]
struct Vertex {
    pub position: [f32; 2],
}

#[repr(C)]
#[derive(Copy, Clone)]
struct Uniforms {
    sample_count: u32,
}

impl Reshaper {
    /// Construct a new `Reshaper`.
    pub fn new(
        device: &wgpu::Device,
        src_texture: &wgpu::TextureView,
        src_sample_count: u32,
        dst_sample_count: u32,
        dst_format: wgpu::TextureFormat,
    ) -> Self {
        // Load shader modules.
        let vs = include_bytes!("shaders/vert.spv");
        let vs_spirv = wgpu::read_spirv(std::io::Cursor::new(&vs[..]))
            .expect("failed to read hard-coded SPIRV");
        let vs_mod = device.create_shader_module(&vs_spirv);
        let fs = match src_sample_count {
            1 => &include_bytes!("shaders/frag.spv")[..],
            2 => &include_bytes!("shaders/frag_msaa2.spv")[..],
            4 => &include_bytes!("shaders/frag_msaa4.spv")[..],
            8 => &include_bytes!("shaders/frag_msaa8.spv")[..],
            16 => &include_bytes!("shaders/frag_msaa16.spv")[..],
            _ => &include_bytes!("shaders/frag_msaa.spv")[..],
        };
        let fs_spirv =
            wgpu::read_spirv(std::io::Cursor::new(fs)).expect("failed to read hard-coded SPIRV");
        let fs_mod = device.create_shader_module(&fs_spirv);

        // Create the sampler for sampling from the source texture.
        let sampler = wgpu::SamplerBuilder::new().build(device);

        // Create the render pipeline.
        let bind_group_layout = bind_group_layout(device, src_sample_count);
        let pipeline_layout = pipeline_layout(device, &bind_group_layout);
        let render_pipeline = render_pipeline(
            device,
            &pipeline_layout,
            &vs_mod,
            &fs_mod,
            dst_sample_count,
            dst_format,
        );

        // Create the uniform buffer to pass the sample count if we don't have an unrolled resolve
        // fragment shader for it.
        let uniform_buffer = match unrolled_sample_count(src_sample_count) {
            true => None,
            false => {
                let uniforms = Uniforms {
                    sample_count: src_sample_count,
                };
                let buffer = device
                    .create_buffer_mapped(1, wgpu::BufferUsage::UNIFORM)
                    .fill_from_slice(&[uniforms]);
                Some(buffer)
            }
        };

        // Create the bind group.
        let bind_group = bind_group(
            device,
            &bind_group_layout,
            src_texture,
            &sampler,
            uniform_buffer.as_ref(),
        );

        // Create the vertex buffer.
        let vertex_buffer = device
            .create_buffer_mapped(VERTICES.len(), wgpu::BufferUsage::VERTEX)
            .fill_from_slice(&VERTICES[..]);

        Reshaper {
            _vs_mod: vs_mod,
            _fs_mod: fs_mod,
            bind_group_layout,
            bind_group,
            render_pipeline,
            sampler,
            uniform_buffer,
            vertex_buffer,
        }
    }

    /// Given an encoder, submits a render pass command for writing the source texture to the
    /// destination texture.
    pub fn encode_render_pass(
        &self,
        dst_texture: &wgpu::TextureView,
        encoder: &mut wgpu::CommandEncoder,
    ) {
        let vertex_range = 0..VERTICES.len() as u32;
        let instance_range = 0..1;

        let render_pass_desc = wgpu::RenderPassDescriptor {
            color_attachments: &[wgpu::RenderPassColorAttachmentDescriptor {
                attachment: dst_texture,
                resolve_target: None,
                load_op: wgpu::LoadOp::Clear,
                store_op: wgpu::StoreOp::Store,
                clear_color: wgpu::Color::TRANSPARENT,
            }],
            depth_stencil_attachment: None,
        };
        let mut render_pass = encoder.begin_render_pass(&render_pass_desc);
        render_pass.set_pipeline(&self.render_pipeline);
        render_pass.set_vertex_buffers(0, &[(&self.vertex_buffer, 0)]);
        render_pass.set_bind_group(0, &self.bind_group, &[]);
        render_pass.draw(vertex_range, instance_range);
    }
}

const VERTICES: [Vertex; 4] = [
    Vertex {
        position: [-1.0, -1.0],
    },
    Vertex {
        position: [-1.0, 1.0],
    },
    Vertex {
        position: [1.0, -1.0],
    },
    Vertex {
        position: [1.0, 1.0],
    },
];

// We provide pre-prepared fragment shaders with unrolled resolves for common sample counts.
fn unrolled_sample_count(sample_count: u32) -> bool {
    match sample_count {
        1 | 2 | 4 | 8 | 16 => true,
        _ => false,
    }
}

fn vertex_attrs() -> [wgpu::VertexAttributeDescriptor; 1] {
    [wgpu::VertexAttributeDescriptor {
        format: wgpu::VertexFormat::Float2,
        offset: 0,
        shader_location: 0,
    }]
}

fn bind_group_layout(device: &wgpu::Device, src_sample_count: u32) -> wgpu::BindGroupLayout {
    let texture_binding = wgpu::BindGroupLayoutBinding {
        binding: 0,
        visibility: wgpu::ShaderStage::FRAGMENT,
        ty: wgpu::BindingType::SampledTexture {
            multisampled: src_sample_count > 1,
            dimension: wgpu::TextureViewDimension::D2,
        },
    };
    let sampler_binding = wgpu::BindGroupLayoutBinding {
        binding: 1,
        visibility: wgpu::ShaderStage::FRAGMENT,
        ty: wgpu::BindingType::Sampler,
    };
    let uniforms_binding = match unrolled_sample_count(src_sample_count) {
        true => None,
        false => Some(wgpu::BindGroupLayoutBinding {
            binding: 2,
            visibility: wgpu::ShaderStage::FRAGMENT,
            ty: wgpu::BindingType::UniformBuffer { dynamic: false },
        }),
    };
    let bindings = match uniforms_binding {
        None => vec![texture_binding, sampler_binding],
        Some(uniforms_binding) => vec![texture_binding, sampler_binding, uniforms_binding],
    };
    let desc = wgpu::BindGroupLayoutDescriptor {
        bindings: &bindings,
    };
    device.create_bind_group_layout(&desc)
}

fn bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    texture: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
    uniform_buffer: Option<&wgpu::Buffer>,
) -> wgpu::BindGroup {
    let texture_binding = wgpu::Binding {
        binding: 0,
        resource: wgpu::BindingResource::TextureView(&texture),
    };
    let sampler_binding = wgpu::Binding {
        binding: 1,
        resource: wgpu::BindingResource::Sampler(&sampler),
    };
    let uniforms_binding = uniform_buffer.map(|buffer| wgpu::Binding {
        binding: 2,
        resource: wgpu::BindingResource::Buffer {
            buffer,
            range: 0..std::mem::size_of::<Uniforms>() as wgpu::BufferAddress,
        },
    });
    let bindings = match uniforms_binding {
        None => vec![texture_binding, sampler_binding],
        Some(uniforms_binding) => vec![texture_binding, sampler_binding, uniforms_binding],
    };
    let desc = wgpu::BindGroupDescriptor {
        layout,
        bindings: &bindings,
    };
    device.create_bind_group(&desc)
}

fn pipeline_layout(
    device: &wgpu::Device,
    bind_group_layout: &wgpu::BindGroupLayout,
) -> wgpu::PipelineLayout {
    let desc = wgpu::PipelineLayoutDescriptor {
        bind_group_layouts: &[&bind_group_layout],
    };
    device.create_pipeline_layout(&desc)
}

fn render_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    vs_mod: &wgpu::ShaderModule,
    fs_mod: &wgpu::ShaderModule,
    dst_sample_count: u32,
    dst_format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let vs_desc = wgpu::ProgrammableStageDescriptor {
        module: &vs_mod,
        entry_point: "main",
    };
    let fs_desc = wgpu::ProgrammableStageDescriptor {
        module: &fs_mod,
        entry_point: "main",
    };
    let raster_desc = wgpu::RasterizationStateDescriptor {
        front_face: wgpu::FrontFace::Ccw,
        cull_mode: wgpu::CullMode::None,
        depth_bias: 0,
        depth_bias_slope_scale: 0.0,
        depth_bias_clamp: 0.0,
    };
    let color_state_desc = wgpu::ColorStateDescriptor {
        format: dst_format,
        color_blend: wgpu::BlendDescriptor::REPLACE,
        alpha_blend: wgpu::BlendDescriptor::REPLACE,
        write_mask: wgpu::ColorWrite::ALL,
    };
    let vertex_attrs = vertex_attrs();
    let vertex_buffer_desc = wgpu::VertexBufferDescriptor {
        stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
        step_mode: wgpu::InputStepMode::Vertex,
        attributes: &vertex_attrs[..],
    };
    let desc = wgpu::RenderPipelineDescriptor {
        layout,
        vertex_stage: vs_desc,
        fragment_stage: Some(fs_desc),
        rasterization_state: Some(raster_desc),
        primitive_topology: wgpu::PrimitiveTopology::TriangleStrip,
        color_states: &[color_state_desc],
        depth_stencil_state: None,
        index_format: wgpu::IndexFormat::Uint16,
        vertex_buffers: &[vertex_buffer_desc],
        sample_count: dst_sample_count,
        sample_mask: !0,
        alpha_to_coverage_enabled: false,
    };
    device.create_render_pipeline(&desc)
}
