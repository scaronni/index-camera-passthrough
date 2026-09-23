#version 450

// Resample one camera image as seen by the rectified pinhole camera.
layout(binding = 0) uniform Parameters {
    // Rotation from rays of the rectified camera to rays of the physical camera,
    // in the upper left 3x3.
    mat4 rotation;
    // Fisheye distortion coefficients
    vec4 dcoef;
    // Optical center, divided by the image size
    vec2 center;
    // Focal length, divided by the image size
    vec2 focal;
    // Offset of this camera in the side by side input texture
    vec2 texOffset;
    // Focal length of the rectified image, divided by its size
    float rectifiedFocal;
};
layout(binding = 1) uniform sampler2D inputTex;

// Output coordinates -0.5 ~ 0.5, x right, y down, relative to the center of the
// rectified image, which is its optical center.
layout(location = 0) in noperspective vec2 coord;
layout(location = 0) out vec4 outColor;
void main() {
    vec3 ray = mat3(rotation) * vec3(coord / rectifiedFocal, 1.0);
    float r = length(ray.xy);
    // Angle from the optical axis, and its distorted value on the sensor.
    float theta = atan(r, ray.z);
    float theta2 = theta * theta;
    float thetaD = theta * (1 + theta2 * (dcoef.x +
                                theta2 * (dcoef.y +
                                theta2 * (dcoef.z +
                                theta2 * dcoef.w))));
    vec2 mapped = r > 0.0 ? ray.xy * (thetaD / r) : vec2(0.0);
    mapped = mapped * focal + center;
    if (any(lessThan(mapped, vec2(0.0))) || any(greaterThan(mapped, vec2(1.0)))) {
        outColor = vec4(0.0, 0.0, 0.0, 1.0);
        return;
    }
    // The input is two images side by side.
    mapped.x *= 0.5;
    outColor = texture(inputTex, mapped + texOffset);
}
