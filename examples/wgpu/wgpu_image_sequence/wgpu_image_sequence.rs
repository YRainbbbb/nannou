//! A demonstration of playing back a sequence of images.
//!
//! This approach loads a directory of images into a single texture array. We only ever present a
//! single layer of the texture at a time by creating a texture view. We select which layer to view
//! by using a `current_layer` variable and updating it based on a frame rate that we determine by
//! the mouse x position.
//!
//! An interesting exercise might be to make a copy of this example and attempt to smooth the slow
//! frame rates by interpolating between two of the layers at a time. Hint: this would likely
//! require adding a second texture view binding to the bind group and its layout.

use nannou::image::RgbaImage;
use nannou::prelude::*;
use std::path::{Path, PathBuf};

struct Model {
    current_layer: f32,
    texture_array: wgpu::Texture,
    texture_view: wgpu::TextureView,
    sampler: wgpu::Sampler,
    bind_group_layout: wgpu::BindGroupLayout,
    bind_group: wgpu::BindGroup,
    render_pipeline: wgpu::RenderPipeline,
    vertex_buffer: wgpu::Buffer,
}

// The vertex type that we will use to represent a point on our triangle.
#[repr(C)]
#[derive(Clone, Copy)]
struct Vertex {
    position: [f32; 2],
}

// The vertices that make up the rectangle to which the image will be drawn.
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

fn main() {
    nannou::app(model).update(update).run();
}

fn model(app: &App) -> Model {
    // Load the images.
    let sequence_path = app
        .assets_path()
        .unwrap()
        .join("images")
        .join("spinning_dancer");

    println!("Loading images...");
    let (images, (img_w, img_h)) = load_images(&sequence_path);
    println!("Done!");

    let w_id = app
        .new_window()
        .size(img_w, img_h)
        .view(view)
        .build()
        .unwrap();

    let window = app.window(w_id).unwrap();
    let device = window.swap_chain_device();
    let format = Frame::TEXTURE_FORMAT;
    let msaa_samples = window.msaa_samples();

    let vs = include_bytes!("shaders/vert.spv");
    let vs_spirv =
        wgpu::read_spirv(std::io::Cursor::new(&vs[..])).expect("failed to read hard-coded SPIRV");
    let vs_mod = device.create_shader_module(&vs_spirv);
    let fs = include_bytes!("shaders/frag.spv");
    let fs_spirv =
        wgpu::read_spirv(std::io::Cursor::new(&fs[..])).expect("failed to read hard-coded SPIRV");
    let fs_mod = device.create_shader_module(&fs_spirv);

    let texture_array = {
        // The wgpu device queue used to load the image data.
        let mut queue = window.swap_chain_queue().lock().unwrap();
        // Describe how we will use the texture so that the GPU may handle it efficiently.
        let usage = wgpu::TextureUsage::SAMPLED;
        let iter = images.iter().map(|&(_, ref img)| img);
        wgpu::Texture::load_array_from_image_buffers(device, &mut *queue, usage, iter)
            .expect("tied to load texture array with an empty image buffer sequence")
    };
    let texture_view = create_layer_texture_view(&texture_array, 0);

    // Create the sampler for sampling from the source texture.
    let sampler = wgpu::SamplerBuilder::new().build(device);

    let bind_group_layout = create_bind_group_layout(device);
    let bind_group = create_bind_group(device, &bind_group_layout, &texture_view, &sampler);
    let pipeline_layout = create_pipeline_layout(device, &bind_group_layout);
    let render_pipeline = create_render_pipeline(
        device,
        &pipeline_layout,
        &vs_mod,
        &fs_mod,
        format,
        msaa_samples,
    );

    // Create the vertex buffer.
    let vertex_buffer = device
        .create_buffer_mapped(VERTICES.len(), wgpu::BufferUsage::VERTEX)
        .fill_from_slice(&VERTICES[..]);

    Model {
        current_layer: 0.0,
        texture_array,
        texture_view,
        sampler,
        bind_group_layout,
        bind_group,
        vertex_buffer,
        render_pipeline,
    }
}

fn update(app: &App, model: &mut Model, update: Update) {
    // Update which layer in the texture array that are viewing.
    let window = app.main_window();
    let device = window.swap_chain_device();

    // Determine how fast to play back the frames based on the mouse x.
    let win_rect = window.rect();
    let fps = map_range(
        app.mouse.x,
        win_rect.left(),
        win_rect.right(),
        -100.0,
        100.0,
    );

    // Update which layer we are viewing based on the playback speed and layer count.
    let layer_count = model.texture_array.array_layer_count();
    model.current_layer = fmod(
        model.current_layer + update.since_last.secs() as f32 * fps,
        layer_count as f32,
    );

    // Update the view and the bind group ready for drawing.
    model.texture_view =
        create_layer_texture_view(&model.texture_array, model.current_layer as u32);
    model.bind_group = create_bind_group(
        device,
        &model.bind_group_layout,
        &model.texture_view,
        &model.sampler,
    );
}

fn view(_app: &App, model: &Model, frame: Frame) {
    let mut encoder = frame.command_encoder();

    let render_pass_desc = wgpu::RenderPassDescriptor {
        color_attachments: &[wgpu::RenderPassColorAttachmentDescriptor {
            attachment: frame.texture_view(),
            resolve_target: None,
            load_op: wgpu::LoadOp::Clear,
            store_op: wgpu::StoreOp::Store,
            clear_color: wgpu::Color::TRANSPARENT,
        }],
        depth_stencil_attachment: None,
    };

    let mut render_pass = encoder.begin_render_pass(&render_pass_desc);
    render_pass.set_bind_group(0, &model.bind_group, &[]);
    render_pass.set_pipeline(&model.render_pipeline);
    render_pass.set_vertex_buffers(0, &[(&model.vertex_buffer, 0)]);

    let vertex_range = 0..VERTICES.len() as u32;
    let instance_range = 0..1;
    render_pass.draw(vertex_range, instance_range);
}

// Load a directory of images and returns them sorted by filename alongside their dimensions.
// This function assumes all the images have the same dimensions.
fn load_images(dir: &Path) -> (Vec<(PathBuf, RgbaImage)>, (u32, u32)) {
    let mut images = vec![];
    let mut dims = (0, 0);
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        let image = match image::open(&path) {
            Ok(img) => img.into_rgba(),
            Err(err) => {
                eprintln!("failed to open {} as an image: {}", path.display(), err);
                continue;
            }
        };
        let (w, h) = image.dimensions();
        dims = (w, h);
        images.push((path, image));
    }
    images.sort_by_key(|(path, _)| path.clone());
    (images, dims)
}

// Create a view of a single layer of a texture array.
fn create_layer_texture_view(texture: &wgpu::Texture, layer: u32) -> wgpu::TextureView {
    let mut desc = texture.create_default_view_descriptor();
    desc.dimension = wgpu::TextureViewDimension::D2;
    desc.base_array_layer = layer;
    desc.array_layer_count = 1;
    texture.create_view(&desc)
}

fn create_bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    let texture_binding = wgpu::BindGroupLayoutBinding {
        binding: 0,
        visibility: wgpu::ShaderStage::FRAGMENT,
        ty: wgpu::BindingType::SampledTexture {
            multisampled: false,
            dimension: wgpu::TextureViewDimension::D2,
        },
    };
    let sampler_binding = wgpu::BindGroupLayoutBinding {
        binding: 1,
        visibility: wgpu::ShaderStage::FRAGMENT,
        ty: wgpu::BindingType::Sampler,
    };
    let bindings = &[texture_binding, sampler_binding];
    let desc = wgpu::BindGroupLayoutDescriptor { bindings };
    device.create_bind_group_layout(&desc)
}

fn create_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    texture: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    let texture_binding = wgpu::Binding {
        binding: 0,
        resource: wgpu::BindingResource::TextureView(&texture),
    };
    let sampler_binding = wgpu::Binding {
        binding: 1,
        resource: wgpu::BindingResource::Sampler(&sampler),
    };
    let bindings = &[texture_binding, sampler_binding];
    let desc = wgpu::BindGroupDescriptor { layout, bindings };
    device.create_bind_group(&desc)
}

fn create_pipeline_layout(
    device: &wgpu::Device,
    bind_group_layout: &wgpu::BindGroupLayout,
) -> wgpu::PipelineLayout {
    let desc = wgpu::PipelineLayoutDescriptor {
        bind_group_layouts: &[&bind_group_layout],
    };
    device.create_pipeline_layout(&desc)
}

fn vertex_attrs() -> [wgpu::VertexAttributeDescriptor; 1] {
    [wgpu::VertexAttributeDescriptor {
        format: wgpu::VertexFormat::Float2,
        offset: 0,
        shader_location: 0,
    }]
}

fn create_render_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    vs_mod: &wgpu::ShaderModule,
    fs_mod: &wgpu::ShaderModule,
    dst_format: wgpu::TextureFormat,
    sample_count: u32,
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
        sample_count,
        sample_mask: !0,
        alpha_to_coverage_enabled: false,
    };
    device.create_render_pipeline(&desc)
}
