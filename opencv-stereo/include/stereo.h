#pragma once

#include <memory>

#include <opencv2/calib3d.hpp>
#include <opencv2/imgproc.hpp>

#include "rust/cxx.h"

namespace icp {

// Rectifies a side by side stereo frame and computes the disparity of the left image.
class StereoMatcher {
  public:
    StereoMatcher(int camera_size, int source_size, int size, rust::Slice<const float> left_map,
                  rust::Slice<const float> right_map, int min_disparity, int num_disparities);

    void compute(rust::Slice<const uint8_t> frame, rust::Slice<int16_t> disparity);

  private:
    int camera_size;
    int source_size;
    int size;
    // x and y maps, for the left and the right image.
    cv::Mat maps[2][2];
    cv::Mat downscaled[2];
    cv::Mat rectified[2];
    cv::Ptr<cv::CLAHE> clahe;
    cv::Ptr<cv::StereoSGBM> sgbm;
};

std::unique_ptr<StereoMatcher> new_stereo_matcher(int camera_size, int source_size, int size,
                                                  rust::Slice<const float> left_map,
                                                  rust::Slice<const float> right_map,
                                                  int min_disparity, int num_disparities);

} // namespace icp
