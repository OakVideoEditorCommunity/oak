add_executable(olive-gtest
  main.cpp
  common_current_test.cpp
  common_xmlutils_test.cpp
  config_test.cpp
  node_value_test.cpp
  node_keyframe_test.cpp
  node_serialization_test.cpp
  render_videoparams_test.cpp
  render_videoparams_branch_test.cpp
  render_audioparams_test.cpp
  render_audioparams_branch_test.cpp
  render_sampleformat_test.cpp
  render_pixelformat_test.cpp
  project_serializer_test.cpp
  timeline_marker_test.cpp
  undo_stack_test.cpp
  plugin_support_test.cpp
  plugin_support_image_test.cpp
  plugin_support_clip_test.cpp
  plugin_support_param_test.cpp
  opengl_readback_guard_test.cpp
  plugin_render_pipeline_test.cpp
  plugin_renderer_readback_test.cpp
  plugin_ofx_integration_test.cpp
  codec_frame_test.cpp
  codec_exportcodec_test.cpp
  codec_exportformat_test.cpp
  codec_encoder_test.cpp
  task_taskmanager_test.cpp
  module_smoke_test.cpp
  shader_resources_test.cpp
  timebased_widget_test.cpp
  timeline_coordinate_test.cpp
  timeline_workarea_test.cpp
)

target_sources(olive-gtest PRIVATE $<TARGET_OBJECTS:libolive-editor>)

target_include_directories(
  olive-gtest
  PRIVATE
  ${CMAKE_SOURCE_DIR}/app
  ${CMAKE_SOURCE_DIR}/tests
  ${CMAKE_SOURCE_DIR}/third_party/openfx/include
  ${CMAKE_SOURCE_DIR}/third_party/openfx/HostSupport/include
  ${OLIVE_INCLUDE_DIRS}
)

target_link_libraries(
  olive-gtest
  PRIVATE
  ${OLIVE_LIBRARIES}
  GTest::gtest
)

target_compile_definitions(
  olive-gtest
  PRIVATE
  ${OLIVE_DEFINITIONS}
)

target_compile_options(
  olive-gtest
  PRIVATE
  ${OLIVE_COMPILE_OPTIONS}
)

if (MSVC)
  add_test("Olive.gtest" olive-gtest)
else()
  add_test(olive-gtest olive-gtest)
endif()
