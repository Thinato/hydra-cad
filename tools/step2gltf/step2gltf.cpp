// step2gltf: convert a STEP file (Z-up, mm) into a binary glTF (.glb, Y-up, metres).
//
//   step2gltf <in.step> <out.glb> [--linear MM] [--angular DEG]
//
// CAD point (x, y, z) in the document's length unit becomes glTF (x, z, -y) in metres.

#include <BRepMesh_IncrementalMesh.hxx>
#include <BRep_Tool.hxx>
#include <Message.hxx>
#include <Message_Messenger.hxx>
#include <Message_PrinterOStream.hxx>
#include <Poly_Triangulation.hxx>
#include <RWGltf_CafWriter.hxx>
#include <STEPCAFControl_Reader.hxx>
#include <TColStd_IndexedDataMapOfStringString.hxx>
#include <TDocStd_Document.hxx>
#include <TopExp_Explorer.hxx>
#include <TopoDS.hxx>
#include <XCAFApp_Application.hxx>
#include <XCAFDoc_DocumentTool.hxx>
#include <XCAFDoc_ShapeTool.hxx>

#include <cmath>
#include <cstdlib>
#include <filesystem>
#include <iostream>
#include <string>
#include <system_error>

static int fail(const std::string& message)
{
  std::cerr << "step2gltf: " << message << std::endl;
  return 1;
}

// Parses a strictly positive number; returns false on anything else.
static bool parsePositive(const char* text, double& value)
{
  char* end = nullptr;
  value = std::strtod(text, &end);
  return end != text && *end == '\0' && value > 0.0;
}

int main(int argc, char** argv)
{
  const std::string usage = "usage: step2gltf <in.step> <out.glb> [--linear MM] [--angular DEG]";
  std::string inPath, outPath;
  double linear = 0.1, angularDeg = 20.0;
  for (int i = 1; i < argc; ++i)
  {
    const std::string arg = argv[i];
    if (arg == "--linear" || arg == "--angular")
    {
      double& target = arg == "--linear" ? linear : angularDeg;
      if (i + 1 >= argc || !parsePositive(argv[++i], target))
        return fail(arg + " needs a positive number; " + usage);
    }
    else if (arg.rfind("--", 0) == 0)
      return fail("unknown option " + arg + "; " + usage);
    else if (inPath.empty())
      inPath = arg;
    else if (outPath.empty())
      outPath = arg;
    else
      return fail("too many arguments; " + usage);
  }
  if (outPath.empty())
    return fail(usage);

  // Keep stdout for our own summary line; OCCT's diagnostics are not useful to callers.
  Message::DefaultMessenger()->RemovePrinters(STANDARD_TYPE(Message_PrinterOStream));

  std::error_code ec;
  if (!std::filesystem::is_regular_file(inPath, ec))
    return fail("cannot read " + inPath + ": no such file");

  Handle(TDocStd_Document) doc;
  XCAFApp_Application::GetApplication()->NewDocument("MDTV-XCAF", doc);

  STEPCAFControl_Reader reader;
  reader.SetNameMode(true);
  reader.SetColorMode(true);
  if (reader.ReadFile(inPath.c_str()) != IFSelect_RetDone)
    return fail(inPath + " is not a readable STEP file");
  if (reader.NbRootsForTransfer() == 0)
    return fail(inPath + " contains no shapes");
  if (!reader.Transfer(doc))
    return fail("cannot translate the shapes in " + inPath);

  TDF_LabelSequence roots;
  XCAFDoc_DocumentTool::ShapeTool(doc->Main())->GetFreeShapes(roots);
  if (roots.IsEmpty())
    return fail(inPath + " contains no shapes");

  int solids = 0, triangles = 0;
  for (const TDF_Label& label : roots)
  {
    const TopoDS_Shape shape = XCAFDoc_ShapeTool::GetShape(label);
    BRepMesh_IncrementalMesh(shape, linear, false, angularDeg * std::acos(-1.0) / 180.0, true);
    for (TopExp_Explorer it(shape, TopAbs_SOLID); it.More(); it.Next())
      ++solids;
    for (TopExp_Explorer it(shape, TopAbs_FACE); it.More(); it.Next())
    {
      TopLoc_Location loc;
      const Handle(Poly_Triangulation) mesh = BRep_Tool::Triangulation(TopoDS::Face(it.Current()), loc);
      if (!mesh.IsNull())
        triangles += mesh->NbTriangles();
    }
  }
  if (triangles == 0)
    return fail("meshing " + inPath + " produced no triangles");

  double unitInMetres = 0.001;
  if (!XCAFDoc_DocumentTool::GetLengthUnit(doc, unitInMetres) || unitInMetres <= 0.0)
    unitInMetres = 0.001;

  RWGltf_CafWriter writer(outPath.c_str(), true);
  RWMesh_CoordinateSystemConverter& converter = writer.ChangeCoordinateSystemConverter();
  converter.SetInputLengthUnit(unitInMetres);
  converter.SetInputCoordinateSystem(RWMesh_CoordinateSystem_Zup);
  converter.SetOutputLengthUnit(1.0);
  converter.SetOutputCoordinateSystem(RWMesh_CoordinateSystem_Yup);
  if (!writer.Perform(doc, TColStd_IndexedDataMapOfStringString(), Message_ProgressRange()))
    return fail("cannot write " + outPath);

  const auto bytes = std::filesystem::file_size(outPath, ec);
  std::cout << roots.Length() << " shapes (" << solids << " solids), " << triangles << " triangles, "
            << bytes << " bytes written to " << outPath << std::endl;
  return 0;
}
