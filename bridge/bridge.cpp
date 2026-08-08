#include <cfloat>
#include <cstdlib>
#include <fstream>
#include <iostream>
#include <numbers>
#include <set>
#include <sstream>
#include <stdexcept>
#include <string>
#include <string_view>
#include <unordered_map>

#include "libslic3r/Format/3mf.hpp"
#include "libslic3r/Model.hpp"
#include "libslic3r/Orient.hpp"
#include "libslic3r/PrintConfig.hpp"
#include "nlohmann/json.hpp"

namespace {
using nlohmann::json;
using namespace Slic3r;

const char* option_type(ConfigOptionType type) {
  switch (type) {
  case coFloat: return "float";
  case coInt: return "int";
  case coString: return "string";
  case coPercent: return "percent";
  case coFloatOrPercent: return "float_or_percent";
  case coPoint: return "point";
  case coPoint3: return "point3";
  case coBool: return "bool";
  case coEnum: return "enum";
  case coFloats:
  case coInts:
  case coStrings:
  case coPercents:
  case coFloatsOrPercents:
  case coPoints:
  case coBools:
  case coEnums:
  case coPointsGroups:
  case coIntsGroups: return "array";
  default: return "unknown";
  }
}

const char* option_mode(ConfigOptionMode mode) {
  switch (mode) {
  case comSimple: return "simple";
  case comAdvanced: return "advanced";
  case comExpert:
  case comDevelop: return "expert";
  }
  return "expert";
}

json default_value(const ConfigOptionDef& definition) {
  const ConfigOption* value = definition.default_value.get();
  if (value == nullptr || value->is_nil()) return nullptr;
  switch (definition.type) {
  case coFloat:
  case coPercent: return value->getFloat();
  case coInt: return value->getInt();
  case coBool: return value->getBool();
  case coString: return static_cast<const ConfigOptionString*>(value)->value;
  case coEnum:
  case coFloatOrPercent:
  case coPoint:
  case coPoint3: return value->serialize();
  default:
    if (value->is_vector())
      return static_cast<const ConfigOptionVectorBase*>(value)->vserialize();
    return value->serialize();
  }
}

json option_json(const std::string& key, const ConfigOptionDef& definition) {
  json option = {
      {"key", key},
      {"type", option_type(definition.type)},
      {"label", definition.full_label.empty() ? definition.label : definition.full_label},
      {"tooltip", definition.tooltip},
      {"mode", option_mode(definition.mode)},
      {"units", definition.sidetext.empty() ? json(nullptr) : json(definition.sidetext)},
      {"nullable", definition.nullable},
      {"default", default_value(definition)},
  };
  if (definition.min != -FLT_MAX) option["min"] = definition.min;
  if (definition.max != FLT_MAX) option["max"] = definition.max;
  if (!definition.enum_values.empty()) option["choices"] = definition.enum_values;
  return option;
}

std::set<std::string> object_option_keys() {
  std::set<std::string> keys;
  const auto append = [&keys](const auto& source) { keys.insert(source.begin(), source.end()); };
  append(PrintObjectConfig().keys());
  append(PrintRegionConfig().keys());
  return keys;
}

int export_schema(const std::string& output_path) {
  json output;
  output["options"] = json::array();
  for (const auto& [key, definition] : print_config_def.options)
    output["options"].push_back(option_json(key, definition));
  output["object_keys"] = object_option_keys();

  std::ofstream stream(output_path, std::ios::binary | std::ios::trunc);
  if (!stream) throw std::runtime_error("cannot open schema output: " + output_path);
  stream << output.dump();
  if (!stream.good()) throw std::runtime_error("cannot write schema output: " + output_path);
  return EXIT_SUCCESS;
}

std::string serialize_override(const json& value) {
  if (value.is_string()) return value.get<std::string>();
  if (value.is_boolean()) return value.get<bool>() ? "1" : "0";
  if (value.is_number()) return value.dump();
  if (value.is_null()) return "nil";
  if (value.is_array()) {
    std::ostringstream stream;
    bool first = true;
    for (const auto& item : value) {
      if (!first) stream << ',';
      first = false;
      stream << serialize_override(item);
    }
    return stream.str();
  }
  throw std::runtime_error("object override must be a scalar or array");
}

Vec3d vector3(const json& value, const char* name) {
  if (!value.is_array() || value.size() != 3)
    throw std::runtime_error(std::string(name) + " must contain three numbers");
  return {value.at(0).get<double>(), value.at(1).get<double>(), value.at(2).get<double>()};
}

std::unordered_map<std::string, ModelObject*> object_map(Model& model) {
  std::unordered_map<std::string, ModelObject*> objects;
  for (ModelObject* object : model.objects) {
    const int backup_id = object->get_backup_id();
    const std::string id = backup_id > 0 ? std::to_string(backup_id)
                                         : std::to_string(object->id().id);
    if (!objects.emplace(id, object).second)
      throw std::runtime_error("model contains duplicate object ID: " + id);
  }
  return objects;
}

int prepare_model(const std::string& input_path, const std::string& state_path,
                  const std::string& output_path) {
  DynamicPrintConfig config;
  ConfigSubstitutionContext substitutions(ForwardCompatibilitySubstitutionRule::Disable);
  Model model = Model::read_from_file(input_path, &config, &substitutions);
  std::ifstream state_stream(state_path);
  if (!state_stream) throw std::runtime_error("cannot open model state: " + state_path);
  const json state = json::parse(state_stream);
  auto objects = object_map(model);
  const std::set<std::string> allowed_overrides = object_option_keys();

  for (const auto& object_state : state.at("objects")) {
    const std::string id = object_state.at("id").get<std::string>();
    const auto found = objects.find(id);
    if (found == objects.end()) throw std::runtime_error("unknown model object ID: " + id);
    ModelObject* object = found->second;
    if (object_state.contains("transform")) {
      const json& transform = object_state.at("transform");
      const Vec3d position = transform.contains("position")
                                 ? vector3(transform.at("position"), "position")
                                 : Vec3d::Zero();
      const Vec3d rotation_degrees = transform.contains("rotation")
                                         ? vector3(transform.at("rotation"), "rotation")
                                         : Vec3d::Zero();
      const Vec3d scale = transform.contains("scale")
                              ? vector3(transform.at("scale"), "scale")
                              : Vec3d::Ones();
      const Vec3d rotation = rotation_degrees * (std::numbers::pi / 180.0);
      for (ModelInstance* instance : object->instances) {
        instance->set_offset(position);
        instance->set_rotation(rotation);
        instance->set_scaling_factor(scale);
      }
    }
    if (object_state.contains("overrides")) {
      for (const auto& [key, value] : object_state.at("overrides").items()) {
        if (!allowed_overrides.contains(key))
          throw std::runtime_error("option is not valid at object scope: " + key);
        object->config.set_deserialize(key, serialize_override(value), substitutions);
      }
    }
  }

  if (state.contains("hidden_object_ids")) {
    for (const auto& value : state.at("hidden_object_ids")) {
      const std::string id = value.get<std::string>();
      const auto found = objects.find(id);
      if (found == objects.end()) throw std::runtime_error("unknown hidden object ID: " + id);
      found->second->printable = false;
    }
  }
  if (state.contains("lay_flat_object_ids")) {
    for (const auto& value : state.at("lay_flat_object_ids")) {
      const std::string id = value.get<std::string>();
      const auto found = objects.find(id);
      if (found == objects.end()) throw std::runtime_error("unknown lay-flat object ID: " + id);
      orientation::orient(found->second);
      found->second->ensure_on_bed();
    }
  }

  if (!store_3mf(output_path.c_str(), &model, &config, false))
    throw std::runtime_error("Orca failed to write prepared 3MF");
  return EXIT_SUCCESS;
}

const char* value_after(int argc, char** argv, std::string_view name) {
  for (int index = 2; index + 1 < argc; ++index)
    if (std::string_view(argv[index]) == name) return argv[index + 1];
  return nullptr;
}
} // namespace

int main(int argc, char** argv) {
  try {
    if (argc >= 2 && std::string_view(argv[1]) == "export-schema") {
      const char* output = value_after(argc, argv, "--output");
      if (output == nullptr)
        throw std::runtime_error("usage: orca-bridge export-schema --output FILE");
      return export_schema(output);
    }
    if (argc >= 2 && std::string_view(argv[1]) == "prepare-model") {
      const char* input = value_after(argc, argv, "--input");
      const char* state = value_after(argc, argv, "--model-state");
      const char* output = value_after(argc, argv, "--output");
      if (input == nullptr || state == nullptr || output == nullptr)
        throw std::runtime_error(
            "usage: orca-bridge prepare-model --input MODEL --model-state STATE --output FILE");
      return prepare_model(input, state, output);
    }
    throw std::runtime_error("usage: orca-bridge export-schema|prepare-model");
  } catch (const std::exception& error) {
    std::cerr << error.what() << '\n';
    return EXIT_FAILURE;
  }
}
