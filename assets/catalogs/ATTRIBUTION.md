# Astronomical Catalogs

## Deep-Sky Catalog (`dso.bin.zst`)

Built from [OpenNGC](https://github.com/mattiaverga/OpenNGC) (`NGC.csv` and `addendum.csv`) by Mattia Verga, licensed under [CC BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/).

OpenNGC incorporates data from:
- NASA/IPAC Extragalactic Database (NED), operated by JPL/Caltech under contract with NASA.
- HyperLEDA database (`http://leda.univ-lyon1.fr`).

Includes the complete Messier catalog (M1–M110) and NGC catalog, packed into Apogee's zstd-compressed `APDS` binary format by `crates/catalog_builder` (`catalog_builder dso`).

## Star Catalog (`stars.bin.zst`)

Built from:
- ESA Gaia DR3 (`gaiadr3.gaia_source`) via the ESA Gaia TAP service.
- Hipparcos New Reduction (`public.hipparcos_newreduction`) via ESA Gaia TAP.
- Yale Bright Star Catalog (BSC5) for bright-star supplements and Bayer/Flamsteed designations.
- IAU Working Group on Star Names (IAU-CSN) for official proper star names.
