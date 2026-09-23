#include "opencv-stereo/include/stereo.h"

#include <stdexcept>

namespace icp {

namespace {

constexpr int BLOCK_SIZE = 7;

// Split interleaved x, y coordinates into the two maps used by cv::remap.
void split_map(rust::Slice<const float> map, int size, cv::Mat &x, cv::Mat &y) {
    if (map.size() != static_cast<size_t>(size) * size * 2) {
        throw std::invalid_argument("map size does not match the output size");
    }
    cv::Mat interleaved(size, size, CV_32FC2, const_cast<float *>(map.data()));
    cv::Mat channels[2];
    cv::split(interleaved, channels);
    x = channels[0];
    y = channels[1];
}

} // namespace

StereoMatcher::StereoMatcher(int camera_size, int source_size, int size,
                             rust::Slice<const float> left_map,
                             rust::Slice<const float> right_map, int min_disparity,
                             int num_disparities)
    : camera_size(camera_size), source_size(source_size), size(size) {
    split_map(left_map, size, maps[0][0], maps[0][1]);
    split_map(right_map, size, maps[1][0], maps[1][1]);
    clahe = cv::createCLAHE(3.0, cv::Size(8, 8));
    sgbm = cv::StereoSGBM::create(min_disparity, num_disparities, BLOCK_SIZE,
                                  8 * BLOCK_SIZE * BLOCK_SIZE, 64 * BLOCK_SIZE * BLOCK_SIZE, 1,
                                  0, 5, 100, 2, cv::StereoSGBM::MODE_SGBM_3WAY);
}

void StereoMatcher::compute(rust::Slice<const uint8_t> frame, rust::Slice<int16_t> disparity) {
    if (frame.size() != static_cast<size_t>(camera_size) * camera_size * 2) {
        throw std::invalid_argument("frame is not two camera images side by side");
    }
    if (disparity.size() != static_cast<size_t>(size) * size) {
        throw std::invalid_argument("disparity size does not match the output size");
    }
    cv::Mat input(camera_size, camera_size * 2, CV_8UC1, const_cast<uint8_t *>(frame.data()));
    for (int i = 0; i < 2; i++) {
        cv::Mat camera = input(cv::Rect(i * camera_size, 0, camera_size, camera_size));
        // Average down first, remap only samples the source at the output density.
        cv::resize(camera, downscaled[i], cv::Size(source_size, source_size), 0, 0,
                   cv::INTER_AREA);
        cv::remap(downscaled[i], rectified[i], maps[i][0], maps[i][1], cv::INTER_LINEAR,
                  cv::BORDER_CONSTANT);
        clahe->apply(rectified[i], rectified[i]);
    }
    cv::Mat output(size, size, CV_16SC1, disparity.data());
    sgbm->compute(rectified[0], rectified[1], output);
}

std::unique_ptr<StereoMatcher> new_stereo_matcher(int camera_size, int source_size, int size,
                                                  rust::Slice<const float> left_map,
                                                  rust::Slice<const float> right_map,
                                                  int min_disparity, int num_disparities) {
    return std::make_unique<StereoMatcher>(camera_size, source_size, size, left_map, right_map,
                                           min_disparity, num_disparities);
}

} // namespace icp
