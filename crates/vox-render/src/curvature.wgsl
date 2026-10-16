
// ---- Earth-radius curvature (ADR-0012 §5, ADR-0015) ----
//
// Appended to BOTH terrain shaders, shader.wgsl and lod.wgsl, by vox-render at
// shader creation, so the two curve with this one function. They must agree
// exactly: any difference shows as a crack where full resolution meets LOD,
// the one place the player is always looking.
//
// Lowers a render-relative position by d^2 * k, d its horizontal distance from
// the camera and k = 1/2R (`sky.fog_range.z`, from
// `vox_core::curvature_coefficient`; 0 = flat). Visual only: positions used
// for fog, the full-resolution mask and everything on the CPU stay flat.
fn curve(world_rel: vec3<f32>) -> vec3<f32> {
    let d = world_rel.xz - sky.cam_scale.xz;
    return vec3<f32>(world_rel.x, world_rel.y - dot(d, d) * sky.fog_range.z, world_rel.z);
}
