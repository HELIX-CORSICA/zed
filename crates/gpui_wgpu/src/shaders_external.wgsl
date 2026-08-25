// External textures: a quad sampling a texture this renderer does not own.
//
// Concatenated only into the storage-buffer variant. WebGL2 has no storage
// buffers and no external-texture producer, so the pipeline is not created
// there and these entry points must not exist in that module.

// 80 bytes, matching the Rust `ExternalTextureSprite`. The tail padding is
// three scalars, not a `vec3<f32>`: a vec3 aligns to 16 in WGSL, which would
// silently grow the struct to 80 bytes and shear every instance after the
// first.
struct ExternalTextureSprite {
    bounds: Bounds,
    tex_bounds: Bounds,
    content_mask: Bounds,
    corner_radii: Corners,
    opacity: f32,
    pad_0: f32,
    pad_1: f32,
    pad_2: f32,
}

@group(1) @binding(0) var<storage, read> b_external_textures: array<ExternalTextureSprite>;

fn load_external_texture(instance_id: u32) -> ExternalTextureSprite {
    return b_external_textures[instance_id];
}

struct ExternalTextureVarying {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) sprite_id: u32,
    @location(3) clip_distances: vec4<f32>,
}

@vertex
fn vs_external_texture(
    @builtin(vertex_index) vertex_id: u32,
    @builtin(instance_index) instance_id: u32,
) -> ExternalTextureVarying {
    let unit_vertex = vec2<f32>(f32(vertex_id & 1u), 0.5 * f32(vertex_id & 2u));
    let sprite = load_external_texture(instance_id);

    var out = ExternalTextureVarying();
    out.position = to_device_position(unit_vertex, sprite.bounds);
    // UV is where this vertex falls inside `tex_bounds`, not inside the quad:
    // several primitives can window onto one full-viewport texture.
    let point = sprite.bounds.origin + unit_vertex * sprite.bounds.size;
    out.uv = (point - sprite.tex_bounds.origin) / sprite.tex_bounds.size;
    out.sprite_id = instance_id;
    out.clip_distances = distance_from_clip_rect(unit_vertex, sprite.bounds, sprite.content_mask);
    return out;
}

@fragment
fn fs_external_texture(input: ExternalTextureVarying) -> @location(0) vec4<f32> {
    let sample = textureSample(t_sprite, s_sprite, input.uv);
    // Alpha clip after using the derivatives, as the sprite shaders do.
    if (any(input.clip_distances < vec4<f32>(0.0))) {
        return vec4<f32>(0.0);
    }

    let sprite = load_external_texture(input.sprite_id);
    let distance = quad_sdf(input.position.xy, sprite.bounds, sprite.corner_radii);
    return blend_color(sample, sprite.opacity * saturate(0.5 - distance));
}
