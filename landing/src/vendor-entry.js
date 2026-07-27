// vendor-entry.js: the ONLY thing bundled into landing/vendor/w3.js.
//
// The landing hero runs the app's real radar globes, so it needs the app's real
// renderer: three r181 (the exact version in package.json) plus the pmndrs
// postprocessing bloom that @react-three/postprocessing wraps. Bundled as an IIFE
// (not ESM) on purpose: the page is opened over file://, where module scripts are
// fetched in CORS mode against a null origin and blocked. A classic script is not.
//
// Every three export is named rather than `import * as THREE`: a namespace import
// pins the whole library live and esbuild cannot shake it (759KB vs 555KB here).
//
// Rebuild: node landing/build-vendor.mjs
import {
  ACESFilmicToneMapping,
  AdditiveBlending,
  AmbientLight,
  BackSide,
  BufferAttribute,
  BufferGeometry,
  CanvasTexture,
  Color,
  CubeCamera,
  DirectionalLight,
  DoubleSide,
  Float32BufferAttribute,
  Group,
  HalfFloatType,
  IcosahedronGeometry,
  LineBasicMaterial,
  LineLoop,
  LineSegments,
  MathUtils,
  Mesh,
  MeshBasicMaterial,
  MeshPhysicalMaterial,
  PerspectiveCamera,
  PlaneGeometry,
  Points,
  PointsMaterial,
  RingGeometry,
  Scene,
  ShaderMaterial,
  Sprite,
  SpriteMaterial,
  Vector3,
  WebGLCubeRenderTarget,
  WebGLRenderer,
} from 'three';
import { EffectComposer, RenderPass, EffectPass, BloomEffect } from 'postprocessing';

const THREE = {
  ACESFilmicToneMapping, AdditiveBlending, AmbientLight, BackSide, BufferAttribute,
  BufferGeometry, CanvasTexture, Color, CubeCamera, DirectionalLight, DoubleSide,
  Float32BufferAttribute, Group, HalfFloatType, IcosahedronGeometry, LineBasicMaterial,
  LineLoop, LineSegments, MathUtils, Mesh, MeshBasicMaterial, MeshPhysicalMaterial,
  PerspectiveCamera, PlaneGeometry, Points, PointsMaterial, RingGeometry, Scene,
  ShaderMaterial, Sprite, SpriteMaterial, Vector3, WebGLCubeRenderTarget, WebGLRenderer,
};

export { THREE, EffectComposer, RenderPass, EffectPass, BloomEffect };
