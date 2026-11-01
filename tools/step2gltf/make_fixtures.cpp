// make_fixtures: write Hydra's demo STEP models (AP214, mm, Z up) into <out-dir>.
//
//   demo-valve-cetop3.step  ISO 4401-03 valve: origin at the pattern datum, mounting face z = 0, body in +z
//   demo-block.step         manifold block with three mount faces and cartridge cavity C1, in block coordinates
//   demo-cartridge.step     screw-in cartridge: axis +z, seating face z = 0, thread in -z

#include <BRepAlgoAPI_Cut.hxx>
#include <BRepBuilderAPI_MakeFace.hxx>
#include <BRepBuilderAPI_MakePolygon.hxx>
#include <BRepPrimAPI_MakeBox.hxx>
#include <BRepPrimAPI_MakeCylinder.hxx>
#include <BRepPrimAPI_MakePrism.hxx>
#include <Interface_Static.hxx>
#include <Message.hxx>
#include <Message_Messenger.hxx>
#include <Message_PrinterOStream.hxx>
#include <Quantity_Color.hxx>
#include <STEPCAFControl_Writer.hxx>
#include <TDataStd_Name.hxx>
#include <TDocStd_Document.hxx>
#include <XCAFApp_Application.hxx>
#include <XCAFDoc_ColorTool.hxx>
#include <XCAFDoc_DocumentTool.hxx>
#include <XCAFDoc_ShapeTool.hxx>

#include <cmath>
#include <iostream>
#include <string>
#include <vector>

static const Quantity_Color kGreyBlue(0.45, 0.52, 0.62, Quantity_TOC_sRGB);
static const Quantity_Color kNearBlack(0.08, 0.08, 0.09, Quantity_TOC_sRGB);
static const Quantity_Color kAluminium(0.80, 0.81, 0.83, Quantity_TOC_sRGB);

static TopoDS_Shape box(double x0, double y0, double z0, double x1, double y1, double z1)
{
  return BRepPrimAPI_MakeBox(gp_Pnt(x0, y0, z0), gp_Pnt(x1, y1, z1)).Shape();
}

static Handle(TDocStd_Document) newDocument()
{
  Handle(TDocStd_Document) doc;
  XCAFApp_Application::GetApplication()->NewDocument("MDTV-XCAF", doc);
  XCAFDoc_DocumentTool::SetLengthUnit(doc, 0.001);
  return doc;
}

// Adds a named, coloured part to the document and returns its label.
static TDF_Label addPart(const Handle(TDocStd_Document)& doc, const TopoDS_Shape& shape, const char* name,
                         const Quantity_Color& color)
{
  const TDF_Label label = XCAFDoc_DocumentTool::ShapeTool(doc->Main())->AddShape(shape, false);
  TDataStd_Name::Set(label, name);
  XCAFDoc_DocumentTool::ColorTool(doc->Main())->SetColor(label, color, XCAFDoc_ColorSurf);
  return label;
}

static bool writeStep(const Handle(TDocStd_Document)& doc, const std::string& path)
{
  STEPCAFControl_Writer writer;
  Interface_Static::SetCVal("write.step.schema", "AP214IS");
  Interface_Static::SetCVal("write.step.unit", "MM");
  if (!writer.Transfer(doc, STEPControl_AsIs) || writer.Write(path.c_str()) != IFSelect_RetDone)
  {
    std::cerr << "make_fixtures: cannot write " << path << std::endl;
    return false;
  }
  std::cout << "wrote " << path << std::endl;
  return true;
}

// Groups the document's parts into one named assembly.
static void makeAssembly(const Handle(TDocStd_Document)& doc, const char* name, const std::vector<TDF_Label>& parts)
{
  Handle(XCAFDoc_ShapeTool) shapes = XCAFDoc_DocumentTool::ShapeTool(doc->Main());
  const TDF_Label assembly = shapes->NewShape();
  TDataStd_Name::Set(assembly, name);
  for (const TDF_Label& part : parts)
  {
    // Name the instance after its part so glTF nodes carry readable names too.
    Handle(TDataStd_Name) partName;
    part.FindAttribute(TDataStd_Name::GetID(), partName);
    TDataStd_Name::Set(shapes->AddComponent(assembly, part, TopLoc_Location()), partName->Get());
  }
  shapes->UpdateAssemblies();
}

// A cylinder along +z from z0 to z1, on the z axis.
static TopoDS_Shape zCylinder(double diameter, double z0, double z1)
{
  return BRepPrimAPI_MakeCylinder(gp_Ax2(gp_Pnt(0, 0, z0), gp_Dir(0, 0, 1)), diameter / 2, z1 - z0).Shape();
}

static bool makeValve(const std::string& path)
{
  Handle(TDocStd_Document) doc = newDocument();
  const gp_Dir xAxis(1, 0, 0);
  makeAssembly(doc, "demo-valve-cetop3", {
    addPart(doc, box(-4.5, -6, 0, 45, 38, 45), "body", kGreyBlue),
    addPart(doc, BRepPrimAPI_MakeCylinder(gp_Ax2(gp_Pnt(45, 16, 24), xAxis), 16, 45).Shape(), "coil-a", kNearBlack),
    addPart(doc, BRepPrimAPI_MakeCylinder(gp_Ax2(gp_Pnt(-49.5, 16, 24), xAxis), 16, 45).Shape(), "coil-b", kNearBlack),
    addPart(doc, box(60, 6, 38, 80, 26, 52), "connector-a", kNearBlack),
    addPart(doc, box(-34.5, 6, 38, -14.5, 26, 52), "connector-b", kNearBlack),
  });
  return writeStep(doc, path);
}

// Fits cavity C1 of the demo block: the thread sits in its 18 mm bore, the nose in its 14 mm bore.
static bool makeCartridge(const std::string& path)
{
  const double pi = std::acos(-1.0);
  BRepBuilderAPI_MakePolygon outline;
  for (int k = 0; k < 6; ++k)
  {
    const double a = k * pi / 3, r = 12 / std::cos(pi / 6);  // 24 mm across flats
    outline.Add(gp_Pnt(r * std::cos(a), r * std::sin(a), 0));
  }
  outline.Close();
  const TopoDS_Shape hex = BRepPrimAPI_MakePrism(BRepBuilderAPI_MakeFace(outline.Wire()).Face(), gp_Vec(0, 0, 15)).Shape();
  Handle(TDocStd_Document) doc = newDocument();
  makeAssembly(doc, "demo-cartridge", {
    addPart(doc, zCylinder(13.5, -55, -30), "nose", kAluminium),
    addPart(doc, zCylinder(17.5, -30, 0), "thread", kAluminium),
    addPart(doc, hex, "hex", kGreyBlue),
    addPart(doc, zCylinder(22, 15, 55), "adjuster", kNearBlack),
  });
  return writeStep(doc, path);
}

struct Hole
{
  double u, v, diameter, depth;
};

struct Face
{
  gp_Pnt origin;
  gp_Dir normal, xDir;
  std::vector<Hole> holes;
};

static bool makeBlock(const std::string& path)
{
  // Hole positions are approximate demo values, not the ISO 4401 standard dimensions.
  const std::vector<Hole> cetop3 = {
    {21.5, 25.9, 7, 15}, {21.5, 5.1, 7, 15}, {12.7, 15.5, 7, 15}, {30.2, 15.5, 7, 15},  // P, T, A, B
    {0, 0, 4.2, 12}, {40.5, -0.75, 4.2, 12}, {40.5, 31.75, 4.2, 12}, {0, 31, 4.2, 12},  // bolts
  };
  const std::vector<Hole> cetop5 = {
    {27, 6.3, 11, 15}, {16.7, 21.4, 11, 15}, {37.3, 21.4, 11, 15},  // P, A, B
    {3.2, 21.4, 11, 15}, {50.8, 21.4, 11, 15},                       // T, T2
    {0, 0, 5, 12}, {54, 0, 5, 12}, {0, 46, 5, 12}, {54, 46, 5, 12},  // bolts
  };
  const std::vector<Hole> c1 = {{0, 0, 18, 32}, {0, 0, 14, 62}};  // stepped cartridge bore
  const Face faces[] = {
    {gp_Pnt(45, 10, 70), gp_Dir(0, 0, 1), gp_Dir(1, 0, 0), cetop3},   // F1
    {gp_Pnt(45, 62, 70), gp_Dir(0, 0, 1), gp_Dir(1, 0, 0), cetop3},   // F2
    {gp_Pnt(30, 0, 15), gp_Dir(0, -1, 0), gp_Dir(1, 0, 0), cetop5},   // F3
    {gp_Pnt(130, 55, 35), gp_Dir(1, 0, 0), gp_Dir(0, 1, 0), c1},      // C1
  };

  // One cut per hole: overlapping tools in a single cut (the stepped bore) leave stray solids.
  TopoDS_Shape block = box(0, 0, 0, 130, 110, 70);
  for (const Face& face : faces)
  {
    const gp_Dir yDir = face.normal.Crossed(face.xDir);
    for (const Hole& hole : face.holes)
    {
      // Start 1 mm outside the face so the cut never leaves a zero-thickness skin.
      const gp_Pnt start = face.origin.Translated(gp_Vec(face.xDir) * hole.u + gp_Vec(yDir) * hole.v + gp_Vec(face.normal));
      const gp_Ax2 axis(start, face.normal.Reversed());
      BRepAlgoAPI_Cut cut(block, BRepPrimAPI_MakeCylinder(axis, hole.diameter / 2, hole.depth + 1).Shape());
      if (!cut.IsDone())
      {
        std::cerr << "make_fixtures: drilling the demo block failed" << std::endl;
        return false;
      }
      block = cut.Shape();
    }
  }
  Handle(TDocStd_Document) doc = newDocument();
  addPart(doc, block, "block", kAluminium);
  return writeStep(doc, path);
}

int main(int argc, char** argv)
{
  if (argc != 2)
  {
    std::cerr << "usage: make_fixtures <out-dir>" << std::endl;
    return 1;
  }
  Message::DefaultMessenger()->RemovePrinters(STANDARD_TYPE(Message_PrinterOStream));
  const std::string dir = argv[1];
  const bool ok = makeValve(dir + "/demo-valve-cetop3.step") && makeBlock(dir + "/demo-block.step") &&
                  makeCartridge(dir + "/demo-cartridge.step");
  return ok ? 0 : 1;
}
