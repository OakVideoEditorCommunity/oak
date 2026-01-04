# Olive Project File Reference

This document describes Olive's XML project format as implemented in the current codebase. It is intended to be detailed enough to implement a compatible reader/writer.

> Source of truth: `app/node/project.cpp`, `app/node/node.cpp`, `app/node/value.*`, `app/node/keyframe.*`, `app/node/project/serializer/*`.

## 1. Root Document

```xml
<olive version="230220" url="/path/to/project.ove">
  ...
</olive>
```

- `version`: serializer version in `YYMMDD` format (latest is `230220`).
- `url`: optional source path.

## 2. Project Container

For full saves, the serializer writes a project container:

```xml
<project>
  <project>...</project>
  <layout>...</layout>
</project>
```

- Inner `<project>` stores actual project data.
- `<layout>` stores UI layout (`MainWindowLayoutInfo::fromXml`).

## 3. Project Data (`Project::Save`)

```xml
<project version="1">
  <uuid>...</uuid>
  <plugins>...</plugins>
  <nodes>...</nodes>
  <settings>...</settings>
</project>
```

### 3.1 `uuid`
- QUuid string.

### 3.2 `plugins`
List of OpenFX plugins referenced by nodes in the project. This is used during load to rescan plugin paths **before** nodes are instantiated.

```xml
<plugins>
  <plugin id="com.vendor.Plugin" major="1" minor="2"
          bundle="/path/to/Plugin.ofx.bundle"
          file="/path/to/Plugin.ofx.bundle/Contents/MacOS/Plugin" />
</plugins>
```

Attributes:
- `id`: OFX plugin identifier (matches node `id`).
- `major` / `minor`: OFX plugin version.
- `bundle`: bundle directory path (preferred).
- `file`: plugin binary path (fallback).

Loading behavior:
- If `plugins` exists, Olive adds each `bundle` (or `file` if `bundle` empty) to the OFX plugin path, scans, then registers plugin nodes before parsing `<nodes>`.

### 3.3 `nodes`
The node graph. Each `<node>` is written by `Node::Save()` and read by `Node::Load()`.

```xml
<nodes version="1">
  <node version="1" id="node.id" ptr="123456">
    <label>Optional Label</label>
    <color>3</color>
    <input>...</input>
    <links>...</links>
    <connections>...</connections>
    <hints>...</hints>
    <context>...</context>
    <caches>...</caches>
    <custom>...</custom>
  </node>
</nodes>
```

Attributes:
- `id`: node type identifier. For OpenFX nodes, this equals the OFX plugin identifier.
- `ptr`: numeric pointer ID used to resolve connections and context positions.
- `version`: currently `1`.

### 3.4 `settings`
Project settings stored as key/value text elements.

Known keys from code:
- `cachesetting`
- `customcachepath`
- `colorconfigfilename`
- `defaultinputcolorspace`
- `colorreferencespace`
- `root` (pointer id of the root Folder node)

## 4. Node Serialization (`Node::Save` / `Node::Load`)

### 4.1 `label`
User-visible node label.

### 4.2 `color`
Override color index (integer), only written if not `-1`.

### 4.3 `input`
Each input is serialized as:

```xml
<input id="InputId">
  <primary>...</primary>
  <subelements count="N">
    <element>...</element>
  </subelements>
</input>
```

- `primary`: element `-1` (the main input).
- `subelements`: array elements (if input is an array). `count` is the array size.

#### 4.3.1 Immediate Values (`primary` / `element`)
Each immediate block contains:

```xml
<keyframing>0|1</keyframing>
<standard>
  <track>...</track>
  ...
</standard>
<keyframes>
  <track>
    <key ...>...</key>
  </track>
</keyframes>
<csinput>...</csinput>
<csdisplay>...</csdisplay>
<csview>...</csview>
<cslook>...</cslook>
```

- `keyframing`: whether input is keyframed (only written if input is keyframable).
- `standard`: default/static values (one `<track>` per keyframe track).
- `keyframes`: only written if `keyframing` is true.
- `cs*`: only written for `kColor` inputs (color management tags).

#### 4.3.2 Track Count
Track count is determined by `NodeValue::get_number_of_keyframe_tracks()`:

| Type | Tracks |
| --- | --- |
| kVec2 | 2 |
| kVec3 | 3 |
| kVec4 | 4 |
| kColor | 4 |
| kBezier | 6 |
| other | 1 |

#### 4.3.3 Standard Value Encoding
Values are written with `NodeValue::ValueToString()` and read with `NodeValue::StringToValue()`.

String encodings:
- `kVec2`: `x:y`
- `kVec3`: `x:y:z`
- `kVec4`: `x:y:z:w`
- `kColor`: `r:g:b:a`
- `kBezier`: `x:y:cp1x:cp1y:cp2x:cp2y`
- `kRational`: `num/den` (see `rational::toString()`)
- `kInt`: integer as text
- `kBinary`: Base64
- `kText`, `kFont`, `kFile`, `kCombo`, `kStrCombo`: string
- `kTexture`, `kSamples`, `kNone`: no text

Special cases:
- `kVideoParams` / `kAudioParams` are nested objects (see section 5).
- `kSubtitleParams` is **skipped on load** to avoid overwriting subtitle data.

#### 4.3.4 Keyframes (`NodeKeyframe::save`)

```xml
<key input="InputId" time="num/den" type="0" inhandlex="0" inhandley="0" outhandlex="0" outhandley="0">value</key>
```

Attributes:
- `input`: input id.
- `time`: rational time (`rational::toString()`).
- `type`: integer enum `NodeKeyframe::Type` (e.g., linear/bezier/etc.).
- `inhandlex`, `inhandley`, `outhandlex`, `outhandley`: bezier handle coordinates.

Text value uses `NodeValue::ValueToString(data_type, value, true)`.

### 4.4 `links`
Block-to-block link list (for timeline items):

```xml
<links>
  <link>ptr</link>
</links>
```

### 4.5 `connections`
Input/output connections:

```xml
<connections>
  <connection input="InputId" element="-1">
    <output>ptr</output>
  </connection>
</connections>
```

- `output` is the serialized pointer (`ptr`) of the output node.

### 4.6 `hints` (Value Hints)
Value hints are per-input UI hints:

```xml
<hints>
  <hint input="InputId" element="-1" version="1">
    <types>
      <type>...</type>
    </types>
    <index>0</index>
    <tag>...</tag>
  </hint>
</hints>
```

### 4.7 `context` (Node positions in contexts)

```xml
<context>
  <node ptr="other_node_ptr">
    <x>0</x>
    <y>0</y>
    <expanded>0|1</expanded>
  </node>
</context>
```

### 4.8 `caches`
Node cache UUIDs:

```xml
<caches>
  <audio>uuid</audio>
  <video>uuid</video>
  <thumb>uuid</thumb>
  <waveform>uuid</waveform>
</caches>
```

### 4.9 `custom`
Custom node data. Default `Node::SaveCustom()` writes nothing. Specific node subclasses may override.

## 5. VideoParams
Serialized inside `<standard><track>` for inputs of type `kVideoParams`.

```xml
<width>...</width>
<height>...</height>
<depth>...</depth>
<timebase>num/den</timebase>
<format>int</format>
<channelcount>int</channelcount>
<pixelaspectratio>num/den</pixelaspectratio>
<interlacing>int</interlacing>
<divider>int</divider>
<enabled>0|1</enabled>
<x>float</x>
<y>float</y>
<streamindex>int</streamindex>
<videotype>int</videotype>
<framerate>num/den</framerate>
<starttime>int64</starttime>
<duration>int64</duration>
<premultipliedalpha>0|1</premultipliedalpha>
<colorspace>string</colorspace>
<colorrange>int</colorrange>
```

## 6. AudioParams
Serialized inside `<standard><track>` for inputs of type `kAudioParams`.

```xml
<samplerate>int</samplerate>
<channellayout>uint64</channellayout>
<format>string</format>
<enabled>0|1</enabled>
<streamindex>int</streamindex>
<duration>int64</duration>
<timebase>num/den</timebase>
```

## 7. Keyframes-only / Markers-only / Nodes-only

The serializer can emit partial documents:

- `markers` (timeline markers)
- `keyframes`
- `nodes` (subset for copy/paste)

These are written by `ProjectSerializer230220::Save()` depending on SaveData.

## 8. OpenFX Node Compatibility Notes

- OpenFX nodes are identified by OFX plugin identifier (`Plugin::getIdentifier()`).
- The `plugins` list ensures Olive can locate external plugins before instantiating nodes.
- If a plugin cannot be found at load time, the node cannot be instantiated and will be skipped.

## 9. Versioning

- Root `olive` element `version` controls which serializer is used.
- Newer files may be rejected with `kProjectTooNew` if no serializer exists.

