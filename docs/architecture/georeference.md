# Georeference

How a raster is placed on the Earth: what a binding is, who supplies each
kind, which one wins, and who interprets a coordinate system. Terms are in
[the glossary](../glossary.md).

## Four kinds of binding, in rank order

The globe (`veldmodules/globe/src/overlay.rs`, `enum Binding`) knows four
kinds, and a later one of a **higher** kind replaces an earlier one; a later
one of a lower kind is ignored. Rank, not arrival order: an overlay has two
rasters, the quicklook and the detailed one, described independently, and
which describes first is the timing of two file parses — nothing meaningful.

| Kind | What it is | Who supplies it |
|---|---|---|
| Catalogue | the footprint of the product: which piece of the Earth was imaged, not which pixel goes where; its vertex order owes nothing to the raster's walk | the catalogue, through `data-provider` and `data-browser` (the quad of the imagery response) |
| Named | the UTM frame derived from the product's name (an MGRS tile): exact, but promised by the name, not said by the file | `data-provider` (`veldmodules/data-provider/src/mgrs.rs`, used by the imagery response in `cdse.rs`) |
| Projected | the projection written in the raster itself: a code and six affine numbers, linear by definition | `image-tiler` (`Placement`): GeoTIFF geokeys (`adapters/tiff.rs`), the GMLJP2 box of a JPEG 2000 (`adapters/jp2.rs`) |
| Lattice | a grid of tie points from the raster, denser than four nodes: it describes a **non-linear** layout — a radar scene lies in the geometry of its acquisition — which no six numbers can | `image-tiler` (`ties`): GeoTIFF tie points, the coordinate file of a NetCDF (`adapters/netcdf.rs`) |

A lattice of exactly four nodes describes nothing non-linear — between four
corners the same linear layout is interpolated — and does not outrank a
projection (`Grid::is_dense`). Everything that leaves the tiler is in WGS84
degrees or in the raster's own projection; the half pixel of
`RasterPixelIsPoint` is removed at the source, so a node and a corner mean
the same thing to the consumer.

## Who interprets a coordinate system

The tiler does not: `Placement` is the EPSG code as written in the file plus
the affine transform, and "zone 38 north" is already an interpretation. The
one thing the tiler asks of a code is the axis order of a GMLJP2 grid
(`easting_first` in `adapters/jp2.rs`, a list of codes whose first axis is
easting), because the grid's two offset vectors carry no axis names. The
globe interprets (`veldmodules/globe/src/projection.rs`): EPSG 3857 is Web
Mercator, computed on the sphere; UTM zones (326xx north, 327xx south) and
Gauss-Krüger zones (284xx, on the Pulkovo 1942 datum) are one transverse
cylindrical projection with different numbers — scale, central meridian,
false offsets, ellipsoid. A code the globe does not know places the raster by
its catalogue footprint, not by a guess. Datums are translated inside the
projection module and only WGS84 leaves it: a latitude on Krasovsky's ellipsoid
looks like one on WGS84 and differs by a hundred metres in silence. The seven
parameters of Pulkovo 1942 are those of GOST R 51794-2008, because the local
systems the cadastre is kept in are described with them.

**MSK-61** (the local system of Rostov oblast, zones 1–3) exists as a branch
of `System::from_epsg` verified by a test, not by a raster: the codes 63361xx
are not EPSG's own, a GeoTIFF geokey is sixteen bits wide and cannot carry a
seven-digit number, and the local system is usually written nowhere in a
file. It waits for whoever can name it with a number.

## When a binding cannot be read

A GeoTIFF or a NetCDF that speaks of a binding the tiler could not take
answers with `binding_trouble`, the reason in words: a tie that is not a
place, a degenerate matrix, a foreign datum, a projected lattice. A JPEG 2000
does not: a GML grid it cannot use is silence, and the raster falls back to
the catalogue footprint without a word. The consumer shows a reason next to
the layer,
because by an empty binding it cannot tell "the file says nothing" from "the
file said something we could not read", and a scene that falls back to the
catalogue footprint would look real. A later success of the other raster of
the overlay clears the complaint (`Overlay::relay`); reasons from two files
accumulate and are not repeated.
