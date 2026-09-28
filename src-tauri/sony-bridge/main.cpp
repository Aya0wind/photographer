// Photo Hub's original adapter. SDK headers and binaries stay outside the repository.
#include <windows.h>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <sstream>
#include <vector>
#include <mutex>
#include <condition_variable>
#include <chrono>
#include <thread>
#include <algorithm>
#include <cstring>
#include "CameraRemote_SDK.h"
#include "IDeviceCallback.h"
using namespace SCRSDK;
namespace fs = std::filesystem;
std::mutex outputMutex;
std::string utf8(const wchar_t* s) {
    if (!s) return "";
    int n=WideCharToMultiByte(CP_UTF8,0,s,-1,nullptr,0,nullptr,nullptr);
    std::string out(n,0); WideCharToMultiByte(CP_UTF8,0,s,-1,out.data(),n,nullptr,nullptr);
    if (!out.empty()) out.pop_back(); return out;
}
std::wstring wide(const std::string& s) {
    int n=MultiByteToWideChar(CP_UTF8,0,s.data(),(int)s.size(),nullptr,0);
    std::wstring out(n,0); MultiByteToWideChar(CP_UTF8,0,s.data(),(int)s.size(),out.data(),n);return out;
}
std::string quote(const std::string& s) {
    std::ostringstream o; o<<'"'; for (unsigned char c:s) {
        if(c=='"'||c=='\\') o<<'\\'<<c;
        else if(c<32) { const char* h="0123456789abcdef";o<<"\\u00"<<h[c>>4]<<h[c&15]; }
        else o<<c;
    }o<<'"';return o.str();
}
void output(const std::string& s) { std::lock_guard<std::mutex> g(outputMutex);std::cout<<s<<std::endl; }
void checked(CrError e) { if(e!=CrError_None) {std::ostringstream o;o<<"Sony SDK error 0x"<<std::hex<<e;throw std::runtime_error(o.str());} }
std::string cameraId(const ICrCameraObjectInfo* c) {
    std::ostringstream o; o<<"sony-sdk:"; const char* h="0123456789abcdef";
    for(unsigned i=0;i<c->GetIdSize();++i){auto v=c->GetId()[i];o<<h[v>>4]<<h[v&15];}return o.str();
}
struct Callback : IDeviceCallback {
    std::mutex mutex;std::condition_variable cv; bool connected=false;std::string error;fs::path incoming;
    void OnConnected(DeviceConnectionVersioin) override {std::lock_guard<std::mutex> g(mutex);connected=true;cv.notify_all();}
    void OnDisconnected(CrInt32u e) override {std::lock_guard<std::mutex> g(mutex);connected=false;error="Camera disconnected: "+std::to_string(e);cv.notify_all();output("{\"type\":\"disconnected\"}");}
    void OnError(CrInt32u e) override {output("{\"type\":\"cameraError\",\"error\":"+quote("Sony SDK error "+std::to_string(e))+"}");}
    void OnCompleteDownload(CrChar* filename,CrInt32u) override {
        auto path=fs::path(filename);if(!path.is_absolute())path=incoming/path;
        output("{\"type\":\"photo\",\"path\":"+quote(utf8(path.c_str()))+"}");
    }
};
struct Properties {
    CrDeviceHandle handle;CrDeviceProperty* data=nullptr;CrInt32 size=0;
    Properties(CrDeviceHandle h):handle(h){checked(GetDeviceProperties(h,&data,&size));}
    ~Properties(){if(data)ReleaseDeviceProperties(handle,data);}
    CrDeviceProperty* find(unsigned code){for(int i=0;i<size;++i)if(data[i].GetCode()==code)return &data[i];return nullptr;}
};
unsigned propertyCode(const std::string& id) {
    if(id=="aperture")return CrDeviceProperty_FNumber;
    if(id=="shutter")return CrDeviceProperty_ShutterSpeed;
    if(id=="iso")return CrDeviceProperty_IsoSensitivity;
    throw std::runtime_error("Unknown camera setting");
}
std::vector<uint64_t> choices(CrDeviceProperty& p) {
    unsigned type=p.GetValueType()&0xF;unsigned bytes=type==1?1:type==2?2:type==3?4:type==4?8:0;
    std::vector<uint64_t> result;if(!bytes)return result;
    auto values=p.GetSetValues();auto size=p.GetSetValueSize();
    if(!values||!size){values=p.GetValues();size=p.GetValueSize();}
    if(size>65536)return result;
    for(unsigned i=0;values&&i+bytes<=size;i+=bytes){uint64_t value=0;memcpy(&value,values+i,bytes);result.push_back(value);}return result;
}
std::string label(const std::string& id,uint64_t v) {
    std::ostringstream o;
    if(id=="aperture") {if(v>=0xFFFD||!v)return "--";o<<"f/"<<(v/100.0);}
    else if(id=="iso") {auto iso=v&0xFFFFFF;if(iso==0xFFFFFF)return "Auto";o<<iso;}
    else {if(v==0)return "Bulb";if(v==0xFFFFFFFF)return "--";auto num=v>>16;auto den=v&65535;if(!num||!den)return "--";if(num<den)o<<num<<"/"<<den<<"s";else o<<(double(num)/den)<<"s";}return o.str();
}
void setProperty(CrDeviceHandle h,unsigned code,uint64_t value,CrDataType type) {
    CrDeviceProperty p;p.SetCode(code);p.SetValueType(type);p.SetCurrentValue(value);checked(SetDeviceProperty(h,&p));
}
int main(int argc,char** argv) {
    SetDllDirectoryW(fs::path(argv[0]).parent_path().c_str());
    if(!Init()){output("{\"type\":\"result\",\"error\":\"Sony SDK initialization failed\"}");return 1;}
    ICrEnumCameraObjectInfo* cameras=nullptr;CrDeviceHandle handle=0;Callback callback;
    uint64_t originalDestination=0;CrDataType destinationType=CrDataType_UInt16;bool restoreDestination=false;
    try {
        checked(EnumCameraObjects(&cameras,3));
        if(argc>1&&std::string(argv[1])=="--list") {
            std::ostringstream o;o<<"{\"type\":\"result\",\"cameras\":[";
            for(unsigned i=0;i<cameras->GetCount();++i){auto c=cameras->GetCameraObjectInfo(i);if(i)o<<',';o<<"{\"pnpId\":"<<quote(cameraId(c))<<",\"name\":"<<quote(utf8(c->GetModel()))<<",\"capabilities\":{\"fileTransfer\":true,\"standardCapture\":true,\"vendorCaptureNikon\":false,\"objectAddedEvents\":true,\"liveView\":true}}";}o<<"]}";output(o.str());
        } else {
            std::string line;
            while(std::getline(std::cin,line)) {
                std::vector<std::string> args;std::stringstream in(line);std::string arg;while(std::getline(in,arg,'\t'))args.push_back(arg);
                try {
                    if(args.empty())continue;
                    auto cmd=args[0];
                    if(cmd=="connect") {
                        if(args.size()!=3||handle)throw std::runtime_error("Invalid connection request");
                        callback.incoming=fs::path(wide(args[2]));fs::create_directories(callback.incoming);
                        const ICrCameraObjectInfo* selected=nullptr;for(unsigned i=0;i<cameras->GetCount();++i){auto c=cameras->GetCameraObjectInfo(i);if(cameraId(c)==args[1])selected=c;}
                        if(!selected)throw std::runtime_error("Camera not connected");
                        checked(Connect(const_cast<ICrCameraObjectInfo*>(selected),&callback,&handle,CrSdkControlMode_Remote,CrReconnecting_OFF));
                        {std::unique_lock<std::mutex> g(callback.mutex);if(!callback.cv.wait_for(g,std::chrono::seconds(20),[&]{return callback.connected||!callback.error.empty();})||!callback.connected)throw std::runtime_error("Camera connection timed out");}
                        checked(SetSDKProperties(handle, Setting_Key_EnableLiveView, 1));
                        bool ready=false;
                        for(int attempt=0;attempt<60;++attempt) { Properties p(handle);if(p.find(CrDeviceProperty_ShutterSpeed)){ready=true;break;}std::this_thread::sleep_for(std::chrono::milliseconds(100)); }
                        if(!ready)throw std::runtime_error("Camera shooting settings are not ready");
                        checked(SetSaveInfo(handle,const_cast<wchar_t*>(callback.incoming.c_str()),const_cast<wchar_t*>(L"PH_"),-1));
                        {Properties p(handle);if(auto dest=p.find(CrDeviceProperty_StillImageStoreDestination)){originalDestination=dest->GetCurrentValue();destinationType=dest->GetValueType();setProperty(handle,dest->GetCode(),CrStillImageStoreDestination_HostPCAndMemoryCard,destinationType);restoreDestination=true;}}
                        output("{\"type\":\"result\",\"ok\":true}");
                    } else if(cmd=="quit") {output("{\"type\":\"result\",\"ok\":true}");break;}
                    else {
                        if(!handle)throw std::runtime_error("Camera not connected");
                        if(cmd=="settings") {
                            Properties p(handle);std::ostringstream o;o<<"{\"type\":\"result\",\"settings\":[";bool first=true;
                            for(auto id:{"shutter","aperture","iso"}) {auto value=p.find(propertyCode(id));if(!value)continue;auto options=choices(*value);if(!first)o<<',';first=false;
                                o<<"{\"id\":"<<quote(id)<<",\"current\":"<<quote(std::to_string(value->GetCurrentValue()))<<",\"writable\":"<<(value->GetPropertyVariableFlag()&&options.size()>1?"true":"false")<<",\"options\":[";
                                if(std::find(options.begin(),options.end(),value->GetCurrentValue())==options.end())options.insert(options.begin(),value->GetCurrentValue());
                                for(unsigned i=0;i<options.size();++i){if(i)o<<',';o<<"{\"value\":"<<quote(std::to_string(options[i]))<<",\"label\":"<<quote(label(id,options[i]))<<'}';}o<<"]}";
                            }o<<"]}";output(o.str());
                        } else if(cmd=="set") {
                            if(args.size()!=3)throw std::runtime_error("Invalid setting request");Properties p(handle);auto value=p.find(propertyCode(args[1]));if(!value)throw std::runtime_error("Setting not supported");
                            auto options=choices(*value);auto requested=std::stoull(args[2]);if(!value->GetPropertyVariableFlag()||std::find(options.begin(),options.end(),requested)==options.end())throw std::runtime_error("Setting is unavailable in the current camera mode");
                            setProperty(handle,value->GetCode(),requested,value->GetValueType());output("{\"type\":\"result\",\"ok\":true}");
                        } else if(cmd=="capture") {
                            checked(SendCommand(handle,CrCommandId_Release,CrCommandParam_Down));std::this_thread::sleep_for(std::chrono::milliseconds(100));checked(SendCommand(handle,CrCommandId_Release,CrCommandParam_Up));output("{\"type\":\"result\",\"ok\":true}");
                        } else if(cmd=="frame") {
                            CrImageInfo info;checked(GetLiveViewImageInfo(handle,&info));auto size=info.GetBufferSize();if(!size||size>16*1024*1024)throw std::runtime_error("Live view frame unavailable");
                            std::vector<CrInt8u> buffer(size);CrImageDataBlock block;block.SetSize(size);block.SetData(buffer.data());checked(GetLiveViewImage(handle,&block));
                            auto target=callback.incoming/L"live-view.jpg";std::ofstream file(target,std::ios::binary|std::ios::trunc);file.write(reinterpret_cast<char*>(block.GetImageData()),block.GetImageSize());file.close();
                            output("{\"type\":\"result\",\"path\":"+quote(utf8(target.c_str()))+"}");
                        } else throw std::runtime_error("Unknown command");
                    }
                }catch(const std::exception& e){output("{\"type\":\"result\",\"error\":"+quote(e.what())+"}");}
            }
        }
    } catch(const std::exception& e){output("{\"type\":\"result\",\"error\":"+quote(e.what())+"}");}
    if(handle){if(restoreDestination)try{setProperty(handle,CrDeviceProperty_StillImageStoreDestination,originalDestination,destinationType);}catch(...){}Disconnect(handle);std::this_thread::sleep_for(std::chrono::milliseconds(250));ReleaseDevice(handle);}
    if(cameras)cameras->Release();Release();return 0;
}
