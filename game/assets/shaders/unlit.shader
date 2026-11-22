shader "CubeLit" {
    pass "Forward" {
        face "back"
        queue "opaque"
        hlsl {
            [[vk::binding(0, 0)]]
            cbuffer FrameData : register(b0)
            {
                float4x4 viewProjection;
                float4x4 viewMatrix;
                float4x4 shadowMatrices[4];
                float4 cascadeSplits;
                uint cascadeIndex;
                uint framePaddingX;
                uint2 framePadding;
                float4 directionalLightDirection;
                float4 directionalLightColorIntensity;
                float4 cameraPosition;
            };

            #include "common/phong.hlsl"

            [[vk::binding(1, 0)]]
            Texture2DArray<float> shadowMap : register(t0);
            [[vk::binding(2, 0)]]
            SamplerComparisonState shadowSampler : register(s0);

            struct VertexInput
            {
                float3 position : POSITION;
                float4 color : COLOR0;
            };

            struct InstanceInput
            {
                float4 world0 : INSTANCE0;
                float4 world1 : INSTANCE1;
                float4 world2 : INSTANCE2;
                float4 world3 : INSTANCE3;
            };

            struct VertexOutput
            {
                float4 position : SV_Position;
                float4 color : COLOR0;
                float3 worldPosition : TEXCOORD0;
            };

            VertexOutput VSMain(VertexInput input, InstanceInput instance)
            {
                VertexOutput output;
                float4 position = float4(input.position, 1.0);
                float4 worldPosition = position.x * instance.world0
                    + position.y * instance.world1
                    + position.z * instance.world2
                    + position.w * instance.world3;
                output.position = mul(viewProjection, worldPosition);
                output.color = input.color;
                output.worldPosition = worldPosition.xyz;
                return output;
            }

            float4 PSMain(VertexOutput input) : SV_Target0
            {
                float viewDepth = abs(mul(viewMatrix, float4(input.worldPosition, 1.0)).z);
                uint cascade = 0;
                if (viewDepth > cascadeSplits.x) cascade = 1;
                if (viewDepth > cascadeSplits.y) cascade = 2;
                if (viewDepth > cascadeSplits.z) cascade = 3;
                float4 shadowPosition = mul(shadowMatrices[cascade], float4(input.worldPosition, 1.0));
                float3 projected = shadowPosition.xyz / shadowPosition.w;
                float2 uv = projected.xy * 0.5 + 0.5;
                float shadow = shadowMap.SampleCmpLevelZero(
                    shadowSampler,
                    float3(uv, cascade),
                    projected.z - 0.001);
                float3 normal = normalize(cross(ddx(input.worldPosition), ddy(input.worldPosition)));
                float3 viewDirection = normalize(cameraPosition.xyz - input.worldPosition);
                float3 color = ComputePhongLighting(
                    input.color.rgb,
                    normal,
                    viewDirection,
                    32.0,
                    shadow);
                return float4(color, input.color.a);
            }
        }
    }

    pass "ShadowCascade" {
        face "front"
        queue "opaque"
        hlsl {
            [[vk::binding(0, 0)]]
            cbuffer FrameData : register(b0)
            {
                float4x4 viewProjection;
                float4x4 viewMatrix;
                float4x4 shadowMatrices[4];
                float4 cascadeSplits;
                uint cascadeIndex;
                uint framePaddingX;
                uint2 framePadding;
            };

            struct VertexInput
            {
                float3 position : POSITION;
                float4 color : COLOR0;
            };

            struct InstanceInput
            {
                float4 world0 : INSTANCE0;
                float4 world1 : INSTANCE1;
                float4 world2 : INSTANCE2;
                float4 world3 : INSTANCE3;
            };

            float4 VSMain(VertexInput input, InstanceInput instance) : SV_Position
            {
                float4 position = float4(input.position, 1.0);
                float4 worldPosition = position.x * instance.world0
                    + position.y * instance.world1
                    + position.z * instance.world2
                    + position.w * instance.world3;
                return mul(shadowMatrices[cascadeIndex], worldPosition);
            }

            void PSMain()
            {
            }
        }
    }
}
