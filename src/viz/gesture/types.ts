// Pure gesture-recognition types shared by the pinch recognizer and the orbit
// adapter. NO three.js / React / MediaPipe imports live here (or in any file in
// this folder) so the whole module is testable in plain node/jsdom.
//
// `HandLandmark` is one MediaPipe Hands normalized landmark: x/y are in [0,1]
// image space (origin top-left), z is relative depth (negative = toward camera).
// A "hand" is exactly 21 of these, indexed per the MediaPipe Hands topology
// (see LANDMARK_INDEX in pinchRecognition.ts).
export interface HandLandmark {
  x: number;
  y: number;
  z: number;
}

// One active pinch this frame. `point` is the smoothed thumb/index midpoint in
// the same normalized [0,1] space as the landmarks; `depth` is the smoothed
// average z of the two fingertips. `id` is the stable per-hand identifier
// (the hand's index in the per-frame hands array).
export interface Pinch {
  id: number;
  point: { x: number; y: number };
  depth: number;
}
