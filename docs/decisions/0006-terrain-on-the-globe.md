# 0006 — Terrain on the globe

Status: accepted (2026-09-06). Written down from the analysis of 2026-08-20.

## Context

A digital elevation model on the globe looks like the next step towards
accuracy: the protocol already carries a float texture (`TEX_R32_FLOAT`), and
the mesh could be lifted by it. The question it is meant to answer is whether
imagery would land more precisely on the surface. The camera is always nadir
(`camera.rs`: the view direction is the surface normal, no tilt by gesture or
by framing), so relief could only shift pixels away from the frame centre,
by the view angle at the edge. Sentinel-2 L1C and L2A are orthorectified by
ESA on a DEM before they reach the catalogue: a pixel already stands at its
own latitude and longitude. Sentinel-1 GRD is the one product where relief
moves the data itself, and moving it back needs the incidence angles of
`annotation/*.xml`, which no module reads.

## Decision

No terrain in the mesh. Relief that helps an agronomist is slope, aspect and
runoff as numbers over the flat view of a field — a layer of its own, not the
geometry of the sphere.

## Rejected

Lifting the base mesh by a DEM. The base mesh is 48 by 96 facets
(`LAT_STEP_DEG`, `LON_STEP_DEG`): a facet spans 3.75° each way, and the sagitta
of its diagonal chord is 6.8 km (R·(1 − cos(2.65°)), computed 2026-09-06) —
the mesh itself sags deeper than any relief on Earth, so a DEM on it means a
new subdivision with levels of detail first. Terrain correction of
Sentinel-1 GRD: the shift is Δz·cot(incidence), 1.0–1.7 km per 1000 m at the
29°–46° of the IW swath (computed from the swath geometry, 2026-08-20); it
needs the annotation angles and is a decoder task, not a mesh one.

## Consequences

Accuracy questions go to the binding and to the camera floor (0007), not to
the mesh. A field-level relief layer, when it comes, is computed from a DEM
over the field polygon and drawn as numbers or shading — it does not touch
`mesh.rs`.
